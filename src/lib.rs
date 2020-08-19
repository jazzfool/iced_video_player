use ffmpeg_next as ffmpeg;
use iced::{image, time, Command, Image, Subscription};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoPlayerMessage {
    NextFrame,
    BufferingComplete {
        fully_buffered: bool,
        packet_count: usize,
    },
}

/// Video player component which can playback videos from files or URLs.
pub struct VideoPlayer {
    /// When the number of remaining buffered frames goes below this number, buffering automatically begins. Default is 100.
    pub buffer_threshold: usize,
    /// Number of packets (not frames) to read on each buffer. Default is 1000.
    pub buffer_size: usize,
    /// Whether the video is paused or not.
    pub paused: bool,

    frame: Option<image::Handle>,
    path: std::path::PathBuf,
    buffered: Arc<Mutex<Vec<image::Handle>>>,
    buffering: bool,
    fully_buffered: bool,
    current_frame: usize,
    packet_count: usize,

    framerate: f32,
}

impl VideoPlayer {
    pub fn new<P: AsRef<std::path::Path>>(path: &P) -> Result<Self, ffmpeg::Error> {
        let video_data = VideoData::new(path)?;
        let framerate = video_data
            .decoder
            .frame_rate()
            .expect("failed to get framerate");

        let buffered = Vec::new();

        Ok(VideoPlayer {
            buffer_threshold: 100,
            buffer_size: 1000,
            paused: false,

            frame: None,
            path: path.as_ref().to_owned(),
            buffered: Arc::new(Mutex::new(buffered)),
            buffering: false,
            fully_buffered: false,
            current_frame: 0,
            packet_count: 0,

            framerate: framerate.0 as f32 / framerate.1 as f32,
        })
    }

    fn buffer(&mut self) -> Command<VideoPlayerMessage> {
        assert!(self.buffer_size > 0);

        if !self.fully_buffered && !self.buffering {
            self.buffering = true;
            let path = self.path.clone();
            let buffered = Arc::clone(&self.buffered);
            let buffer_size = self.buffer_size;
            let prev_packet_count = self.packet_count;
            Command::perform(
                tokio::spawn(async move {
                    let mut video_data =
                        VideoData::new(&path).expect("failed to initialize decoder");

                    let mut packet_count = 0;
                    for (stream, packet) in video_data.ictx.packets().skip(prev_packet_count) {
                        if packet_count >= buffer_size {
                            return (false, prev_packet_count + packet_count - 1);
                        }

                        if stream.index() == video_data.video_stream_index {
                            video_data.decoder.send_packet(&packet).unwrap();
                            let mut decoded = ffmpeg::util::frame::Video::empty();
                            while video_data.decoder.receive_frame(&mut decoded).is_ok() {
                                let mut rgb = ffmpeg::util::frame::Video::empty();
                                video_data
                                    .scaler
                                    .run(&decoded, &mut rgb)
                                    .expect("failed to convert frame color space");
                                buffered
                                    .lock()
                                    .expect("failed to unlock buffered frames for buffering")
                                    .push(image::Handle::from_pixels(
                                        rgb.width(),
                                        rgb.height(),
                                        rgb.data(0).to_owned(),
                                    ));
                            }
                        }
                        packet_count += 1;
                    }

                    (true, prev_packet_count + packet_count - 1)
                }),
                |o| {
                    let (fully_buffered, packet_count) = o.expect("async error");
                    VideoPlayerMessage::BufferingComplete {
                        fully_buffered,
                        packet_count,
                    }
                },
            )
        } else {
            Command::none()
        }
    }

    /// Whether buffering is currently taking place in another thread.
    pub fn is_buffering(&self) -> bool {
        self.buffering
    }

    /// Returns the number of buffered frames.
    pub fn buffered_frames_len(&self) -> usize {
        self.buffered
            .lock()
            .expect("failed to lock buffered frames")
            .len()
    }

    /// Returns a list of all the buffered frames as Iced image handles.
    ///
    /// This may block if buffering is occurring.
    pub fn buffered_frames(&self) -> Vec<image::Handle> {
        self.buffered
            .lock()
            .expect("failed to lock buffered frames")
            .clone() // image::Handle data is stored in Arc, so this isn't heavy
    }

    /// Seeks to a specified frame number.
    ///
    /// Panics if `frame >= buffered_frames_len()`
    pub fn seek(&mut self, frame: usize) {
        assert!(frame < self.buffered_frames_len());
        self.current_frame = frame;
    }

    pub fn update(&mut self, message: VideoPlayerMessage) -> Command<VideoPlayerMessage> {
        match message {
            VideoPlayerMessage::NextFrame => {
                if self.paused {
                    return Command::none();
                }

                let (next_frame, len) = {
                    let buffered = self
                        .buffered
                        .lock()
                        .expect("failed to unlock buffered frames");
                    (buffered.get(self.current_frame).cloned(), buffered.len())
                };

                if let Some(img) = next_frame {
                    self.frame = Some(img.clone());

                    if self.current_frame < len - 1 {
                        self.current_frame += 1;
                        if len - self.current_frame < self.buffer_threshold {
                            self.buffer()
                        } else {
                            Command::none()
                        }
                    } else {
                        Command::none()
                    }
                } else {
                    // no more frames
                    self.buffer()
                }
            }
            VideoPlayerMessage::BufferingComplete {
                fully_buffered,
                packet_count,
            } => {
                self.buffering = false;
                self.fully_buffered = fully_buffered;
                self.packet_count = packet_count;
                Command::none()
            }
        }
    }

    pub fn subscription(&self) -> Subscription<VideoPlayerMessage> {
        if !self.paused {
            time::every(Duration::from_secs_f32(1.0 / self.framerate))
                .map(|_| VideoPlayerMessage::NextFrame)
        } else {
            Subscription::none()
        }
    }

    pub fn view(&mut self) -> Image {
        Image::new(
            self.frame
                .clone()
                .unwrap_or_else(|| image::Handle::from_pixels(0, 0, vec![])),
        )
        .into()
    }
}

struct VideoData {
    ictx: ffmpeg::format::context::Input,
    video_stream_index: usize,
    decoder: ffmpeg::codec::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
}

impl VideoData {
    fn new<P: AsRef<std::path::Path>>(path: &P) -> Result<Self, ffmpeg::Error> {
        ffmpeg::init()?;

        let ictx = ffmpeg::format::input(path)?;
        let input = ictx.streams().best(ffmpeg::media::Type::Video).unwrap();
        let video_stream_index = input.index();
        let decoder = input.codec().decoder().video()?;

        let scaler = ffmpeg::software::scaling::Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            ffmpeg::format::Pixel::BGRA,
            decoder.width(),
            decoder.height(),
            ffmpeg::software::scaling::Flags::BILINEAR,
        )?;

        Ok(VideoData {
            ictx,
            video_stream_index,
            decoder,
            scaler,
        })
    }
}
