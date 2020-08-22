use iced::{button, executor, Application, Button, Column, Command, Element, Subscription, Text};
use iced_video_player::{VideoPlayer, VideoPlayerMessage};

fn main() {
    App::run(Default::default());
}

#[derive(Clone, Debug)]
enum Message {
    TogglePause,
    VideoPlayerMessage(VideoPlayerMessage),
}

struct App {
    video: VideoPlayer,
    pause_btn: button::State,
}

impl Application for App {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        (
            App {
                video: VideoPlayer::new(
                    &url::Url::from_file_path(
                        std::path::PathBuf::from(file!())
                            .parent()
                            .unwrap()
                            .join("../.media/test.mp4")
                            .canonicalize()
                            .unwrap(),
                    )
                    .unwrap(),
                )
                .unwrap(),
                pause_btn: Default::default(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::TogglePause => {
                self.video.set_paused(!self.video.paused());
            }
            Message::VideoPlayerMessage(msg) => self.video.update(msg),
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
                Button::new(&mut self.pause_btn, Text::new("Toggle Pause"))
                    .on_press(Message::TogglePause),
            )
            .into()
    }
}
