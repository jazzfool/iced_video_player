use iced::{
    widget::{Button, Column, Row, Slider, Text},
    Element, Sandbox,
};
use iced_video_player::{Video, VideoPlayer};
use std::time::Duration;

fn main() {
    App::run(Default::default()).unwrap();
}

#[derive(Clone, Debug)]
enum Message {
    TogglePause,
    ToggleLoop,
    Seek(f64),
    SeekRelease,
    EndOfStream,
    NewFrame,
}

struct App {
    video: Video,
    position: f64,
    dragging: bool,
}

impl Sandbox for App {
    type Message = Message;

    fn new() -> Self {
        let video = Video::new(
            &url::Url::from_file_path(
                std::path::PathBuf::from(file!())
                    .parent()
                    .unwrap()
                    .join("../.media/test.mp4")
                    .canonicalize()
                    .unwrap(),
            )
            .unwrap(),
            false,
        )
        .unwrap();
        App {
            video,
            position: 0.0,
            dragging: false,
        }
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::TogglePause => {
                self.video.set_paused(!self.video.paused());
            }
            Message::ToggleLoop => {
                self.video.set_looping(!self.video.looping());
            }
            Message::Seek(secs) => {
                self.dragging = true;
                self.video.set_paused(true);
                self.position = secs;
            }
            Message::SeekRelease => {
                self.dragging = false;
                self.video
                    .seek(Duration::from_secs_f64(self.position))
                    .expect("seek");
                self.video.set_paused(false);
            }
            Message::EndOfStream => {
                println!("end of stream");
            }
            Message::NewFrame => {
                if !self.dragging {
                    self.position = self.video.position().as_secs_f64();
                }
            }
        }
    }

    fn view(&self) -> Element<Message> {
        Column::new()
            .push(
                VideoPlayer::new(&self.video)
                    .on_end_of_stream(Message::EndOfStream)
                    .on_new_frame(Message::NewFrame),
            )
            .push(
                Row::new()
                    .spacing(5)
                    .push(
                        Button::new(Text::new(if self.video.paused() {
                            "Play"
                        } else {
                            "Pause"
                        }))
                        .on_press(Message::TogglePause),
                    )
                    .push(
                        Button::new(Text::new(if self.video.looping() {
                            "Disable Loop"
                        } else {
                            "Enable Loop"
                        }))
                        .on_press(Message::ToggleLoop),
                    )
                    .push(Text::new(format!(
                        "{:#?}s / {:#?}s",
                        self.position as u64,
                        self.video.duration().as_secs()
                    )))
                    .push(
                        Slider::new(
                            0.0..=self.video.duration().as_secs_f64(),
                            self.position,
                            Message::Seek,
                        )
                        .step(0.1)
                        .on_release(Message::SeekRelease),
                    ),
            )
            .into()
    }
}
