use crate::Error;
use gstreamer as gst;
use gstreamer_app as gst_app;
use gstreamer_app::prelude::*;
use iced::widget::image as img;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    pub(crate) alive: Arc<AtomicBool>,
    pub(crate) worker: Option<std::thread::JoinHandle<()>>,

    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) framerate: f64,
    pub(crate) duration: std::time::Duration,
    pub(crate) speed: f64,
    pub(crate) sync_av: bool,

    pub(crate) frame: Arc<Mutex<Vec<u8>>>,
    pub(crate) upload_frame: Arc<AtomicBool>,
    pub(crate) last_frame_time: Arc<Mutex<Instant>>,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) looping: bool,
    pub(crate) is_eos: bool,
    pub(crate) restart_stream: bool,
}

impl Internal {
    pub(crate) fn seek(&self, position: impl Into<Position>, accurate: bool) -> Result<(), Error> {
        let position = position.into();
        // gstreamer complains if the start & end value types aren't the same
        let end = match &position {
            Position::Time(_) => Position::Time(std::time::Duration::ZERO),
            Position::Frame(_) => Position::Frame(0),
        };

        self.source.seek(
            self.speed,
            gst::SeekFlags::FLUSH
                | gst::SeekFlags::KEY_UNIT
                | if accurate {
                    gst::SeekFlags::ACCURATE
                } else {
                    gst::SeekFlags::empty()
                },
            gst::SeekType::Set,
            gst::GenericFormattedValue::from(position),
            gst::SeekType::End,
            gst::GenericFormattedValue::from(end),
        )?;
        Ok(())
    }

    pub(crate) fn set_speed(&mut self, speed: f64) -> Result<(), Error> {
        let Some(position) = self.source.query_position::<gst::ClockTime>() else {
            return Err(Error::Caps);
        };
        if speed > 0.0 {
            self.source.seek(
                speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                position,
                gst::SeekType::End,
                gst::ClockTime::from_seconds(0),
            )?;
        } else {
            self.source.seek(
                speed,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_seconds(0),
                gst::SeekType::Set,
                position,
            )?;
        }
        self.speed = speed;
        Ok(())
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.is_eos = false;
        self.set_paused(false);
        self.seek(0, false)?;
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
        self.paused.store(paused, Ordering::SeqCst);

        // Set restart_stream flag to make the stream restart on the next Message::NextFrame
        if self.is_eos && !paused {
            self.restart_stream = true;
        }
    }

    /// Syncs audio with video when there is (inevitably) latency presenting the frame.
    pub(crate) fn set_av_offset(&mut self, offset: Duration) {
        if self.sync_av {
            self.source
                .set_property("av-offset", -(offset.as_nanos() as i64));
        }
    }
}

/// A multimedia video loaded from a URI (e.g., a local file path or HTTP stream).
pub struct Video(pub(crate) RefCell<Internal>);

impl Drop for Video {
    fn drop(&mut self) {
        let inner = self.0.get_mut();

        inner
            .source
            .set_state(gst::State::Null)
            .expect("failed to set state");

        inner.alive.store(false, Ordering::SeqCst);
        if let Some(worker) = inner.worker.take() {
            worker.join().expect("failed to stop video thread");
        }
    }
}

impl Video {
    /// Create a new video player from a given video which loads from `uri`.
    /// Note that live sources will report the duration to be zero.
    pub fn new(uri: &url::Url) -> Result<Self, Error> {
        gst::init()?;

        let pipeline = format!("playbin uri=\"{}\" video-sink=\"videoscale ! videoconvert ! appsink name=iced_video drop=true caps=video/x-raw,format=NV12,pixel-aspect-ratio=1/1\"", uri.as_str());
        let pipeline = gst::parse::launch(pipeline.as_ref())?
            .downcast::<gst::Pipeline>()
            .map_err(|_| Error::Cast)?;

        let video_sink: gst::Element = pipeline.property("video-sink");
        let pad = video_sink.pads().get(0).cloned().unwrap();
        let pad = pad.dynamic_cast::<gst::GhostPad>().unwrap();
        let bin = pad
            .parent_element()
            .unwrap()
            .downcast::<gst::Bin>()
            .unwrap();
        let app_sink = bin.by_name("iced_video").unwrap();
        let app_sink = app_sink.downcast::<gst_app::AppSink>().unwrap();

        Self::from_gst_pipeline(pipeline, app_sink)
    }

    /// Creates a new video based on an existing GStreamer pipeline and appsink.
    /// Expects an `appsink` plugin with `caps=video/x-raw,format=NV12`.
    ///
    /// **Note:** Many functions of [`Video`] assume a `playbin` pipeline.
    /// Non-`playbin` pipelines given here may not have full functionality.
    pub fn from_gst_pipeline(
        pipeline: gst::Pipeline,
        app_sink: gst_app::AppSink,
    ) -> Result<Self, Error> {
        gst::init()?;
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);

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
        // resolution should be mod4
        let width = ((width + 4 - 1) / 4) * 4;
        let framerate = s
            .get::<gst::Fraction>("framerate")
            .map_err(|_| Error::Caps)?;
        let framerate = framerate.numer() as f64 / framerate.denom() as f64;

        if framerate.is_nan()
            || framerate.is_infinite()
            || framerate < 0.0
            || framerate.abs() < f64::EPSILON
        {
            return Err(Error::Framerate(framerate));
        }

        let duration = std::time::Duration::from_nanos(
            pipeline
                .query_duration::<gst::ClockTime>()
                .map(|duration| duration.nseconds())
                .unwrap_or(0),
        );

        let sync_av = pipeline.has_property("av-offset", None);

        // NV12 = 12bpp
        let frame = Arc::new(Mutex::new(vec![
            0u8;
            (width as usize * height as usize * 3)
                .div_ceil(2)
        ]));
        let upload_frame = Arc::new(AtomicBool::new(false));
        let alive = Arc::new(AtomicBool::new(true));
        let last_frame_time = Arc::new(Mutex::new(Instant::now()));
        let paused = Arc::new(AtomicBool::new(false));

        let frame_ref = Arc::clone(&frame);
        let upload_frame_ref = Arc::clone(&upload_frame);
        let alive_ref = Arc::clone(&alive);
        let last_frame_time_ref = Arc::clone(&last_frame_time);
        let paused_ref = Arc::clone(&paused);

        let worker = std::thread::spawn(move || {
            while alive_ref.load(Ordering::Acquire) {
                if let Err(gst::FlowError::Error) = (|| -> Result<(), gst::FlowError> {
                    let sample = if paused_ref.load(Ordering::SeqCst) {
                        app_sink
                            .try_pull_preroll(gst::ClockTime::from_mseconds(16))
                            .ok_or(gst::FlowError::Eos)?
                    } else {
                        app_sink
                            .try_pull_sample(gst::ClockTime::from_mseconds(16))
                            .ok_or(gst::FlowError::Eos)?
                    };

                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let mut frame = frame_ref.lock().map_err(|_| gst::FlowError::Error)?;
                    let frame_len = frame.len();
                    frame.copy_from_slice(&map.as_slice()[..frame_len]);

                    *last_frame_time_ref
                        .lock()
                        .map_err(|_| gst::FlowError::Error)? = Instant::now();
                    upload_frame_ref.swap(true, Ordering::SeqCst);

                    Ok(())
                })() {
                    log::error!("error pulling frame");
                }
            }
        });

        Ok(Video(RefCell::new(Internal {
            id,

            bus: pipeline.bus().unwrap(),
            source: pipeline,
            alive,
            worker: Some(worker),

            width,
            height,
            framerate,
            duration,
            speed: 1.0,
            sync_av,

            frame,
            upload_frame,
            last_frame_time,
            paused,
            looping: false,
            is_eos: false,
            restart_stream: false,
        })))
    }

    /// Get the size/resolution of the video as `(width, height)`.
    pub fn size(&self) -> (i32, i32) {
        (self.0.borrow().width, self.0.borrow().height)
    }

    /// Get the framerate of the video as frames per second.
    pub fn framerate(&self) -> f64 {
        self.0.borrow().framerate
    }

    /// Set the volume multiplier of the audio.
    /// `0.0` = 0% volume, `1.0` = 100% volume.
    ///
    /// This uses a linear scale, for example `0.5` is perceived as half as loud.
    pub fn set_volume(&mut self, volume: f64) {
        self.0.get_mut().source.set_property("volume", volume);
        self.set_muted(self.muted()); // for some reason gstreamer unmutes when changing volume?
    }

    /// Get the volume multiplier of the audio.
    pub fn volume(&self) -> f64 {
        self.0.borrow().source.property("volume")
    }

    /// Set if the audio is muted or not, without changing the volume.
    pub fn set_muted(&mut self, muted: bool) {
        self.0.get_mut().source.set_property("mute", muted);
    }

    /// Get if the audio is muted or not.
    pub fn muted(&self) -> bool {
        self.0.borrow().source.property("mute")
    }

    /// Get if the stream ended or not.
    pub fn eos(&self) -> bool {
        self.0.borrow().is_eos
    }

    /// Get if the media will loop or not.
    pub fn looping(&self) -> bool {
        self.0.borrow().looping
    }

    /// Set if the media will loop or not.
    pub fn set_looping(&mut self, looping: bool) {
        self.0.get_mut().looping = looping;
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        let inner = self.0.get_mut();
        inner.set_paused(paused);
    }

    /// Get if the media is paused or not.
    pub fn paused(&self) -> bool {
        self.0.borrow().paused.load(Ordering::SeqCst)
    }

    /// Jumps to a specific position in the media.
    /// Passing `true` to the `accurate` parameter will result in more accurate seeking,
    /// however, it is also slower. For most seeks (e.g., scrubbing) this is not needed.
    pub fn seek(&mut self, position: impl Into<Position>, accurate: bool) -> Result<(), Error> {
        self.0.get_mut().seek(position, accurate)
    }

    /// Set the playback speed of the media.
    /// The default speed is `1.0`.
    pub fn set_speed(&mut self, speed: f64) -> Result<(), Error> {
        self.0.get_mut().set_speed(speed)
    }

    /// Get the current playback speed.
    pub fn speed(&self) -> f64 {
        self.0.borrow().speed
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
    pub fn duration(&self) -> std::time::Duration {
        self.0.borrow().duration
    }

    /// Restarts a stream; seeks to the first frame and unpauses, sets the `eos` flag to false.
    pub fn restart_stream(&mut self) -> Result<(), Error> {
        self.0.get_mut().restart_stream()
    }

    /// Set the subtitle URL to display.
    pub fn set_subtitle_url(&mut self, url: &url::Url) -> Result<(), Error> {
        let paused = self.paused();
        let inner = self.0.get_mut();
        inner.source.set_state(gst::State::Ready)?;
        inner.source.set_property("suburi", url.as_str());
        inner.set_paused(paused);
        Ok(())
    }

    /// Get the current subtitle URL.
    pub fn subtitle_url(&self) -> Option<url::Url> {
        url::Url::parse(&self.0.borrow().source.property::<String>("suburi")).ok()
    }

    /// Set the font used to display subtitles.
    pub fn set_subtitle_font(&mut self, family: &str, size_pt: u8) {
        self.0
            .get_mut()
            .source
            .set_property("subtitle-font-desc", format!("{}, {}", family, size_pt));
    }

    /// Set whether the subtitle stream is enabled.
    pub fn set_subtitles_enabled(&mut self, enabled: bool) {
        let source = &self.0.get_mut().source;
        let flags = source.property_value("flags");
        let flags_class = glib::FlagsClass::with_type(flags.type_()).unwrap();
        let flags = flags_class.builder_with_value(flags).unwrap();
        let flags = if enabled {
            flags.set_by_nick("text")
        } else {
            flags.unset_by_nick("text")
        }
        .build()
        .unwrap();
        source.set_property_from_value("flags", &flags);
    }

    /// Get whether the subtitle stream is enabled.
    pub fn subtitles_enabled(&self) -> bool {
        let source = &self.0.borrow().source;
        let flags = source.property_value("flags");
        let flags_class = glib::FlagsClass::with_type(flags.type_()).unwrap();
        flags_class.is_set_by_nick(&flags, "text")
    }

    /// Get the underlying GStreamer pipeline.
    pub fn pipeline(&self) -> gst::Pipeline {
        self.0.borrow().source.clone()
    }

    /// Generates a list of thumbnails based on a set of positions in the media.
    ///
    /// Slow; only needs to be called once for each instance.
    /// It's best to call this at the very start of playback, otherwise the position may shift.
    pub fn thumbnails(&mut self, positions: &[Position]) -> Result<Vec<img::Handle>, Error> {
        let paused = self.paused();
        let muted = self.muted();
        let pos = self.position();

        self.set_paused(false);
        self.set_muted(true);

        let out = {
            let inner = self.0.borrow();
            let width = inner.width;
            let height = inner.height;
            positions
                .iter()
                .map(|&pos| {
                    inner.seek(pos, true)?;
                    inner.upload_frame.store(false, Ordering::SeqCst);
                    while !inner.upload_frame.load(Ordering::SeqCst) {
                        std::hint::spin_loop();
                    }
                    Ok(img::Handle::from_rgba(
                        inner.width as _,
                        inner.height as _,
                        yuv_to_rgba(
                            &inner.frame.lock().map_err(|_| Error::Lock)?,
                            width as _,
                            height as _,
                        ),
                    ))
                })
                .collect()
        };

        self.set_paused(paused);
        self.set_muted(muted);
        self.seek(pos, true)?;

        out
    }
}

fn yuv_to_rgba(yuv: &[u8], width: u32, height: u32) -> Vec<u8> {
    let uv_start = width * height;
    let mut rgba = vec![];

    for y in 0..height {
        for x in 0..width {
            let uv_i = uv_start + width * (y / 2) + x / 2 * 2;

            let y = yuv[(y * width + x) as usize] as f32;
            let u = yuv[uv_i as usize] as f32;
            let v = yuv[(uv_i + 1) as usize] as f32;

            let r = 1.164 * (y - 16.0) + 1.596 * (v - 128.0);
            let g = 1.164 * (y - 16.0) - 0.813 * (v - 128.0) - 0.391 * (u - 128.0);
            let b = 1.164 * (y - 16.0) + 2.018 * (u - 128.0);

            rgba.push(r as u8);
            rgba.push(g as u8);
            rgba.push(b as u8);
            rgba.push(0xFF);
        }
    }

    return rgba;
}
