use iced::{
    button, executor, Application, Button, Column, Command, Element, Row, Subscription, Text,
};
use iced_video_player::{VideoPlayer, VideoPlayerMessage};

fn main() {
    App::run(Default::default()).unwrap();
}

#[derive(Clone, Debug)]
enum Message {
    TogglePause,
    ToggleLoop,
    VideoPlayerMessage(VideoPlayerMessage),
}

struct App {
    video: VideoPlayer,
    pause_btn: button::State,
    loop_btn: button::State,
}

impl Application for App {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let video = VideoPlayer::new(
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
        (
            App {
                video,
                pause_btn: Default::default(),
                loop_btn: Default::default(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, message: Message, _: &mut iced::Clipboard) -> Command<Message> {
        match message {
            Message::TogglePause => {
                self.video.set_paused(!self.video.paused());
            }
            Message::ToggleLoop => {
                self.video.set_looping(!self.video.looping());
            }
            Message::VideoPlayerMessage(msg) => {
                return self.video.update(msg).map(Message::VideoPlayerMessage);
            }
        }

        Command::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        self.video.subscription().map(Message::VideoPlayerMessage)
    }

    fn view(&mut self) -> Element<Message> {
        Column::new()
            .push(self.video.frame_view())
            .push(
                Row::new()
                    .spacing(5)
                    .push(
                        Button::new(
                            &mut self.pause_btn,
                            Text::new(if self.video.paused() { "Play" } else { "Pause" }),
                        )
                        .on_press(Message::TogglePause),
                    )
                    .push(
                        Button::new(
                            &mut self.loop_btn,
                            Text::new(if self.video.looping() {
                                "Disable Loop"
                            } else {
                                "Enable Loop"
                            }),
                        )
                        .on_press(Message::ToggleLoop),
                    )
                    .push(Text::new(format!(
                        "{:#?}s / {:#?}s",
                        self.video.position().as_secs(),
                        self.video.duration().as_secs()
                    ))),
            )
            .into()
    }
}
