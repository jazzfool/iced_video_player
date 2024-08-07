use crate::Error;
use gst::prelude::*;
use gst_base::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_base as gst_base;
use iced::widget::image as img;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;

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
            Position::Frame(f) => gst::format::Default::from_u64(f).into(),
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

pub(crate) struct Internal {
    pub(crate) id: u64,

    pub(crate) bus: gst::Bus,
    pub(crate) source: gst::Pipeline,

    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) framerate: f64,
    pub(crate) duration: std::time::Duration,

    pub(crate) frame: Arc<Mutex<Vec<u8>>>, // ideally would be Arc<Mutex<[T]>>
    pub(crate) upload_frame: Arc<AtomicBool>,
    pub(crate) wait: mpsc::Receiver<()>,
    pub(crate) paused: bool,
    pub(crate) muted: bool,
    pub(crate) looping: bool,
    pub(crate) is_eos: bool,
    pub(crate) restart_stream: bool,
    pub(crate) next_redraw: Instant,
}

impl Internal {
    pub(crate) fn seek(&self, position: impl Into<Position>) -> Result<(), Error> {
        self.source.seek_simple(
            gst::SeekFlags::FLUSH,
            gst::GenericFormattedValue::from(position.into()),
        )?;
        Ok(())
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.is_eos = false;
        self.set_paused(false);
        self.seek(0)?;
        Ok(())
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
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
}

/// A multimedia video loaded from a URI (e.g., a local file path or HTTP stream).
pub struct Video(pub(crate) RefCell<Internal>);

impl Drop for Video {
    fn drop(&mut self) {
        self.0
            .borrow()
            .source
            .set_state(gst::State::Null)
            .expect("failed to set state");
    }
}

impl Video {
    /// Create a new video player from a given video which loads from `uri`.
    /// Note that live sourced will report the duration to be zero.
    pub fn new(uri: &url::Url) -> Result<Self, Error> {
        let pipeline = format!("uridecodebin uri=\"{}\" ! videoconvert ! videoscale ! appsink name=iced_video caps=video/x-raw,format=RGBA,pixel-aspect-ratio=1/1", uri.as_str());
        Self::from_pipeline(pipeline)
    }

    pub fn from_pipeline<S: AsRef<str>>(pipeline: S) -> Result<Self, Error> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);

        gst::init()?;

        let pipeline = gst::parse::launch(pipeline.as_ref())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| Error::Cast)?;
        let mut live = false;
        pipeline
            .iterate_sources()
            .foreach(|elem| {
                if let Ok(src) = elem.downcast::<gst_base::BaseSrc>() {
                    if src.is_live() {
                        live = true;
                    }
                }
            })
            .unwrap();

        let app_sink_name = "iced_video";
        let app_sink = pipeline
            .by_name(app_sink_name)
            .and_then(|elem| elem.downcast::<gst_app::AppSink>().ok())
            .ok_or(Error::AppSink(app_sink_name.to_string()))?;

        let pad = app_sink.pads().first().cloned().unwrap();

        pipeline.set_state(gst::State::Playing)?;

        // wait for up to 5 seconds until the decoder gets the source capabilities
        pipeline.state(gst::ClockTime::from_seconds(5)).0?;

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
                pipeline
                    .query_duration::<gst::ClockTime>()
                    .ok_or(Error::Duration)?
                    .nseconds(),
            )
        } else {
            std::time::Duration::from_secs(0)
        };

        let frame_buf = vec![0; (width * height * 4) as _];
        let frame = Arc::new(Mutex::new(frame_buf));
        let frame_ref = Arc::clone(&frame);

        let upload_frame = Arc::new(AtomicBool::new(true));
        let upload_frame_ref = Arc::clone(&upload_frame);

        let (notify, wait) = mpsc::channel();

        app_sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    frame_ref
                        .lock()
                        .map_err(|_| gst::FlowError::Error)?
                        .copy_from_slice(map.as_slice());

                    upload_frame_ref.store(true, Ordering::SeqCst);

                    notify.send(()).map_err(|_| gst::FlowError::Error)?;

                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        Ok(Video(RefCell::new(Internal {
            id,

            bus: pipeline.bus().unwrap(),
            source: pipeline,

            width,
            height,
            framerate: framerate.numer() as f64 / framerate.denom() as f64,
            duration,

            frame,
            upload_frame,
            wait,
            paused: false,
            muted: false,
            looping: false,
            is_eos: false,
            restart_stream: false,
            next_redraw: Instant::now(),
        })))
    }

    /// Get the size/resolution of the video as `(width, height)`.
    #[inline(always)]
    pub fn size(&self) -> (i32, i32) {
        (self.0.borrow().width, self.0.borrow().height)
    }

    /// Get the framerate of the video as frames per second.
    #[inline(always)]
    pub fn framerate(&self) -> f64 {
        self.0.borrow().framerate
    }

    /// Set the volume multiplier of the audio.
    /// `0.0` = 0% volume, `1.0` = 100% volume.
    ///
    /// This uses a linear scale, for example `0.5` is perceived as half as loud.
    pub fn set_volume(&mut self, volume: f64) {
        self.0.borrow().source.set_property("volume", volume);
    }

    /// Set if the audio is muted or not, without changing the volume.
    pub fn set_muted(&mut self, muted: bool) {
        let mut inner = self.0.borrow_mut();
        inner.muted = muted;
        inner.source.set_property("mute", muted);
    }

    /// Get if the audio is muted or not.
    #[inline(always)]
    pub fn muted(&self) -> bool {
        self.0.borrow().muted
    }

    /// Get if the stream ended or not.
    #[inline(always)]
    pub fn eos(&self) -> bool {
        self.0.borrow().is_eos
    }

    /// Get if the media will loop or not.
    #[inline(always)]
    pub fn looping(&self) -> bool {
        self.0.borrow().looping
    }

    /// Set if the media will loop or not.
    #[inline(always)]
    pub fn set_looping(&mut self, looping: bool) {
        self.0.borrow_mut().looping = looping;
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        let mut inner = self.0.borrow_mut();
        inner.set_paused(paused);
    }

    /// Get if the media is paused or not.
    #[inline(always)]
    pub fn paused(&self) -> bool {
        self.0.borrow().paused
    }

    /// Jumps to a specific position in the media.
    /// The seeking is not perfectly accurate.
    pub fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        self.0.borrow_mut().seek(position)
    }

    /// Get the current playback position in time.
    pub fn position(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(
            self.0
                .borrow()
                .source
                .query_position::<gst::ClockTime>()
                .map_or(0, |pos| pos.nseconds()),
        )
    }

    /// Get the media duration.
    #[inline(always)]
    pub fn duration(&self) -> std::time::Duration {
        self.0.borrow().duration
    }

    /// Restarts a stream; seeks to the first frame and unpauses, sets the `eos` flag to false.
    pub fn restart_stream(&mut self) -> Result<(), Error> {
        self.0.borrow_mut().restart_stream()
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
                let inner = self.0.borrow();
                // for some reason waiting for two frames is necessary
                // maybe in a small window between seek and wait the old frame comes in?
                inner.wait.recv().map_err(|_| Error::Sync)?;
                inner.wait.recv().map_err(|_| Error::Sync)?;
                Ok(img::Handle::from_pixels(
                    inner.width as _,
                    inner.height as _,
                    self.0
                        .borrow()
                        .frame
                        .lock()
                        .map_err(|_| Error::Lock)?
                        .clone(),
                ))
            })
            .collect();
        self.set_paused(paused);
        self.seek(pos)?;
        out
    }
}
