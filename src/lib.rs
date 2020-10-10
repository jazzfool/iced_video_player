use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use iced::{image as img, Command, Image, Subscription};
use num_traits::ToPrimitive;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use thiserror::Error;

/// Position in the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Position {
    /// Position based on time.
    ///
    /// Not the most accurate format for videos.
    Time(std::time::Duration),
    /// Position based on nth frame.
    Frame(u64),
}

impl From<Position> for gst::GenericFormattedValue {
    fn from(pos: Position) -> Self {
        match pos {
            Position::Time(t) => gst::ClockTime::from_nseconds(t.as_nanos() as _).into(),
            Position::Frame(f) => gst::format::Default(Some(f)).into(),
        }
    }
}

impl From<std::time::Duration> for Position {
    fn from(t: std::time::Duration) -> Self {
        Position::Time(t)
    }
}

impl From<u64> for Position {
    fn from(f: u64) -> Self {
        Position::Frame(f)
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Glib(#[from] glib::Error),
    #[error("{0}")]
    Bool(#[from] glib::BoolError),
    #[error("failed to get the gstreamer bus")]
    Bus,
    #[error("{0}")]
    StateChange(#[from] gst::StateChangeError),
    #[error("failed to cast gstreamer element")]
    Cast,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid URI")]
    Uri,
    #[error("failed to get media capabilities")]
    Caps,
    #[error("failed to query media duration or position")]
    Duration,
    #[error("failed to sync with playback")]
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoPlayerMessage {
    NextFrame,
    EndOfPlayback,
}

/// Video player which handles multimedia playback.
pub struct VideoPlayer {
    bus: gst::Bus,
    source: gst::Bin,

    width: i32,
    height: i32,
    framerate: f64,
    duration: std::time::Duration,

    frame: Arc<Mutex<Option<img::Handle>>>,
    wait: mpsc::Receiver<()>,
    paused: bool,
    muted: bool,
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.source
            .set_state(gst::State::Null)
            .expect("failed to set state");
    }
}

impl VideoPlayer {
    /// Create a new video player from a given video which loads from `uri`.
    ///
    /// If `live` is set then no duration is queried (as this will result in an error and is non-sensical for live streams).
    /// Set `live` if the streaming source is indefinite (e.g. a live stream).
    /// Note that this will cause the duration to be zero.
    pub fn new(uri: &url::Url, live: bool) -> Result<Self, Error> {
        gst::init()?;

        let source = gst::parse_launch(&format!("playbin uri=\"{}\" video-sink=\"videoconvert ! videoscale ! appsink name=app_sink caps=video/x-raw,format=BGRA,pixel-aspect-ratio=1/1\"", uri.as_str()))?;
        let source = source.downcast::<gst::Bin>().unwrap();

        let video_sink: gst::Element = source
            .get_property("video-sink")
            .unwrap()
            .get()
            .unwrap()
            .unwrap();
        let pad = video_sink.get_pads().get(0).cloned().unwrap();
        let pad = pad.dynamic_cast::<gst::GhostPad>().unwrap();
        let bin = pad
            .get_parent_element()
            .unwrap()
            .downcast::<gst::Bin>()
            .unwrap();

        let app_sink = bin.get_by_name("app_sink").unwrap();
        let app_sink = app_sink.downcast::<gst_app::AppSink>().unwrap();

        let frame = Arc::new(Mutex::new(None));
        let frame_ref = Arc::clone(&frame);

        let (notify, wait) = mpsc::channel();

        app_sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.get_buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let pad = sink.get_static_pad("sink").ok_or(gst::FlowError::Error)?;

                    let caps = pad.get_current_caps().ok_or(gst::FlowError::Error)?;
                    let s = caps.get_structure(0).ok_or(gst::FlowError::Error)?;
                    let width = s
                        .get::<i32>("width")
                        .map_err(|_| gst::FlowError::Error)?
                        .ok_or(gst::FlowError::Error)?;
                    let height = s
                        .get::<i32>("height")
                        .map_err(|_| gst::FlowError::Error)?
                        .ok_or(gst::FlowError::Error)?;

                    *frame_ref.lock().map_err(|_| gst::FlowError::Error)? =
                        Some(img::Handle::from_pixels(
                            width as _,
                            height as _,
                            map.as_slice().to_owned(),
                        ));

                    notify.send(()).map_err(|_| gst::FlowError::Error)?;

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        source.set_state(gst::State::Playing)?;

        // wait for up to 5 seconds until the decoder gets the source capabilities
        source.get_state(gst::ClockTime::from_seconds(5)).0?;

        // extract resolution and framerate
        // TODO(jazzfool): maybe we want to extract some other information too?
        let caps = pad.get_current_caps().ok_or(Error::Caps)?;
        let s = caps.get_structure(0).ok_or(Error::Caps)?;
        let width = s
            .get::<i32>("width")
            .map_err(|_| Error::Caps)?
            .ok_or(Error::Caps)?;
        let height = s
            .get::<i32>("height")
            .map_err(|_| Error::Caps)?
            .ok_or(Error::Caps)?;
        let framerate = s
            .get::<gst::Fraction>("framerate")
            .map_err(|_| Error::Caps)?
            .ok_or(Error::Caps)?;

        let duration = if !live {
            std::time::Duration::from_nanos(
                source
                    .query_duration::<gst::ClockTime>()
                    .ok_or(Error::Duration)?
                    .nanoseconds()
                    .ok_or(Error::Duration)?,
            )
        } else {
            std::time::Duration::from_secs(0)
        };

        Ok(VideoPlayer {
            bus: source.get_bus().unwrap(),
            source,

            width,
            height,
            framerate: num_rational::Rational::new(
                *framerate.numer() as _,
                *framerate.denom() as _,
            )
            .to_f64().unwrap(/* if the video framerate is bad then it would've been implicitly caught far earlier */),
            duration,

            frame,
            wait,
            paused: false,
            muted: false,
        })
    }

    /// Get the size/resolution of the video as `(width, height)`.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Get the framerate of the video as frames per second.
    pub fn framerate(&self) -> f64 {
        self.framerate
    }

    /// Set the volume multiplier of the audio.
    /// `0.0` = 0% volume, `1.0` = 100% volume.
    ///
    /// This uses a linear scale, for example `0.5` is perceived as half as loud.
    pub fn set_volume(&mut self, volume: f64) {
        self.source.set_property("volume", &volume).unwrap(/* this property is guaranteed to exist */);
    }

    /// Set if the audio is muted or not, without changing the volume.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.source.set_property("mute", &muted).unwrap();
    }

    /// Get if the audio is muted or not.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.source
            .set_state(if paused {
                gst::State::Paused
            } else {
                gst::State::Playing
            })
            .unwrap(/* state was changed in ctor; state errors caught there */);
    }

    /// Get if the media is paused or not.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Jumps to a specific position in the media.
    /// The seeking is not perfectly accurate.
    pub fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        self.source
            .seek_simple(gst::SeekFlags::FLUSH, position.into())?;
        Ok(())
    }

    /// Get the current playback position in time.
    pub fn position(&self) -> Option<std::time::Duration> {
        std::time::Duration::from_nanos(
            self.source
                .query_position::<gst::ClockTime>()?
                .nanoseconds()?,
        )
        .into()
    }

    /// Get the media duration.
    pub fn duration(&self) -> std::time::Duration {
        self.duration
    }

    /// Generates a list of thumbnails based on a set of positions in the media.
    ///
    /// Slow; only needs to be called once for each instance.
    /// It's best to call this at the very start of playback, otherwise the position may shift.
    pub fn thumbnails(&mut self, positions: &[Position]) -> Result<Vec<img::Handle>, Error> {
        let paused = self.paused;
        let pos = self.position().ok_or(Error::Duration)?;
        self.set_paused(false);
        let out = positions
            .iter()
            .map(|&pos| {
                self.seek(pos)?;
                self.wait.recv().map_err(|_| Error::Sync)?;
                Ok(self.frame_image())
            })
            .collect();
        self.set_paused(paused);
        self.seek(pos)?;
        out
    }

    pub fn update(&mut self, message: VideoPlayerMessage) -> Command<VideoPlayerMessage> {
        match message {
            VideoPlayerMessage::NextFrame => {
                let mut cmd = Command::none();
                for msg in self.bus.iter() {
                    match msg.view() {
                        gst::MessageView::Error(err) => panic!("{:#?}", err),
                        gst::MessageView::Eos(_eos) => {
                            cmd = Command::batch(vec![
                                cmd,
                                Command::perform(async {}, |_| VideoPlayerMessage::EndOfPlayback),
                            ])
                        }
                        _ => {}
                    }
                }
                cmd
            }
            _ => Command::none(),
        }
    }

    pub fn subscription(&self) -> Subscription<VideoPlayerMessage> {
        if !self.paused {
            time::every(Duration::from_secs_f64(0.5 / self.framerate))
                .map(|_| VideoPlayerMessage::NextFrame)
        } else {
            Subscription::none()
        }
    }

    /// Get the image handle of the current frame.
    pub fn frame_image(&self) -> img::Handle {
        self.frame
            .lock()
            .expect("failed to lock frame")
            .clone()
            .unwrap_or_else(|| img::Handle::from_pixels(0, 0, vec![]))
    }

    /// Wrap the output of `frame_image` in an `Image` widget.
    pub fn frame_view(&mut self) -> Image {
        Image::new(self.frame_image())
    }
}

// until iced 0.2 is released, which has this built-in
mod time {
    use iced::futures;

    pub fn every(duration: std::time::Duration) -> iced::Subscription<std::time::Instant> {
        iced::Subscription::from_recipe(Every(duration))
    }

    struct Every(std::time::Duration);

    impl<H, I> iced_native::subscription::Recipe<H, I> for Every
    where
        H: std::hash::Hasher,
    {
        type Output = std::time::Instant;

        fn hash(&self, state: &mut H) {
            use std::hash::Hash;

            std::any::TypeId::of::<Self>().hash(state);
            self.0.hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: futures::stream::BoxStream<'static, I>,
        ) -> futures::stream::BoxStream<'static, Self::Output> {
            use futures::stream::StreamExt;

            tokio::time::interval(self.0)
                .map(|_| std::time::Instant::now())
                .boxed()
        }
    }
}
