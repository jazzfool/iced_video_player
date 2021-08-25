use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use iced::{image as img, Command, Image, Subscription};
use num_traits::ToPrimitive;
use std::convert::identity;
use std::future;
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
            Position::Frame(f) => gst::format::Default(f).into(),
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

impl VideoPlayerMessage {
    fn into_cmd(self) -> Command<Self> {
        Command::perform(future::ready(self), identity)
    }
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
    looping: bool,
    is_eos: bool,
    restart_stream: bool,
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

        let video_sink: gst::Element = source.property("video-sink").unwrap().get().unwrap();
        let pad = video_sink.pads().get(0).cloned().unwrap();
        let pad = pad.dynamic_cast::<gst::GhostPad>().unwrap();
        let bin = pad
            .parent_element()
            .unwrap()
            .downcast::<gst::Bin>()
            .unwrap();

        let app_sink = bin.by_name("app_sink").unwrap();
        let app_sink = app_sink.downcast::<gst_app::AppSink>().unwrap();

        let frame = Arc::new(Mutex::new(None));
        let frame_ref = Arc::clone(&frame);

        let (notify, wait) = mpsc::channel();

        app_sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let pad = sink.static_pad("sink").ok_or(gst::FlowError::Error)?;

                    let caps = pad.current_caps().ok_or(gst::FlowError::Error)?;
                    let s = caps.structure(0).ok_or(gst::FlowError::Error)?;
                    let width = s.get::<i32>("width").map_err(|_| gst::FlowError::Error)?;
                    let height = s.get::<i32>("height").map_err(|_| gst::FlowError::Error)?;

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
        source.state(gst::ClockTime::from_seconds(5)).0?;

        // extract resolution and framerate
        // TODO(jazzfool): maybe we want to extract some other information too?
        let caps = pad.current_caps().ok_or(Error::Caps)?;
        let s = caps.structure(0).ok_or(Error::Caps)?;
        let width = s.get::<i32>("width").map_err(|_| Error::Caps)?;
        let height = s.get::<i32>("height").map_err(|_| Error::Caps)?;
        let framerate = s
            .get::<gst::Fraction>("framerate")
            .map_err(|_| Error::Caps)?;

        let duration = if !live {
            std::time::Duration::from_nanos(
                source
                    .query_duration::<gst::ClockTime>()
                    .ok_or(Error::Duration)?
                    .nseconds(),
            )
        } else {
            std::time::Duration::from_secs(0)
        };

        Ok(VideoPlayer {
            bus: source.bus().unwrap(),
            source,

            width,
            height,
            framerate: num_rational::Rational32::new(
                *framerate.numer() as _,
                *framerate.denom() as _,
            )
            .to_f64().unwrap(/* if the video framerate is bad then it would've been implicitly caught far earlier */),
            duration,

            frame,
            wait,
            paused: false,
            muted: false,
            looping: false,
            is_eos: false,
            restart_stream: false,
        })
    }

    /// Get the size/resolution of the video as `(width, height)`.
    #[inline(always)]
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Get the framerate of the video as frames per second.
    #[inline(always)]
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
    #[inline(always)]
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Get if the stream ended or not.
    #[inline(always)]
    pub fn eos(&self) -> bool {
        self.is_eos
    }

    /// Get if the media will loop or not.
    #[inline(always)]
    pub fn looping(&self) -> bool {
        self.looping
    }

    /// Set if the media will loop or not.
    #[inline(always)]
    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        self.source
            .set_state(if paused {
                gst::State::Paused
            } else {
                gst::State::Playing
            })
            .unwrap(/* state was changed in ctor; state errors caught there */);
        self.paused = paused;

        // Set restart_stream flag to make the stream restart on the next Message::NextFrame
        if self.is_eos && !paused {
            self.restart_stream = true;
        }
    }

    /// Get if the media is paused or not.
    #[inline(always)]
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
    pub fn position(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(
            self.source
                .query_position::<gst::ClockTime>()
                .map_or(0, |pos| pos.nseconds()),
        )
        .into()
    }

    /// Get the media duration.
    #[inline(always)]
    pub fn duration(&self) -> std::time::Duration {
        self.duration
    }

    /// Generates a list of thumbnails based on a set of positions in the media.
    ///
    /// Slow; only needs to be called once for each instance.
    /// It's best to call this at the very start of playback, otherwise the position may shift.
    pub fn thumbnails(&mut self, positions: &[Position]) -> Result<Vec<img::Handle>, Error> {
        let paused = self.paused();
        let pos = self.position();
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
                let mut cmds = Vec::new();

                let mut restart_stream = false;
                if self.restart_stream {
                    restart_stream = true;
                    // Set flag to false to avoid potentially multiple seeks
                    self.restart_stream = false;
                }
                let mut eos_pause = false;

                for msg in self.bus.iter() {
                    match msg.view() {
                        gst::MessageView::Error(err) => panic!("{:#?}", err),
                        gst::MessageView::Eos(_eos) => {
                            cmds.push(VideoPlayerMessage::EndOfPlayback.into_cmd());
                            if self.looping {
                                restart_stream = true;
                            } else {
                                eos_pause = true;
                            }
                        }
                        _ => {}
                    }
                }

                // Don't run eos_pause if restart_stream is true; fixes "pausing" after restarting a stream
                if restart_stream {
                    if let Err(err) = self.restart_stream() {
                        eprintln!("cannot restart stream (can't seek): {:#?}", err);
                    }
                } else if eos_pause {
                    self.is_eos = true;
                    self.set_paused(true);
                }

                return Command::batch(cmds);
            }
            VideoPlayerMessage::EndOfPlayback => {}
        }
        Command::none()
    }

    pub fn subscription(&self) -> Subscription<VideoPlayerMessage> {
        if self.restart_stream || (!self.is_eos && !self.paused()) {
            iced::time::every(Duration::from_secs_f64(0.5 / self.framerate))
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

    /// Restarts a stream; seeks to the first frame and unpauses, sets the `eos` flag to false.
    pub fn restart_stream(&mut self) -> Result<(), Error> {
        self.is_eos = false;
        self.set_paused(false);
        self.seek(0)?;
        Ok(())
    }
}
