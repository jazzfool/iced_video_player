# Iced Video Player Widget

Composable component to play videos in any Iced application.

<img src=".media/screenshot.png" width="50%" />

## Overview

Features:
- Load video files from any file path **or URL** (support for streaming over network).
- Non-blocking (off-thread) automatic buffering.
- Programmatic play/pause/jump.
- Small (around 250 lines).

Limitations (to be hopefully fixed):
- Cannot load in-memory video data.
- Audio playback is not supported.
- Buffering does not support seeking arbitrarily - you can only seek to buffered frames.
- FFmpeg is a heavy dependency and overkill (open to recommendations for similar *cross-platform* Rust libraries).

The player **does not** come with any surrounding GUI controls, but they should be quite easy to implement should you need them;
- Play/pause/stop can just be buttons.
- Seeking can be a slider with an overlay of the thumbnail at the seek time.
Specifically, the player exposes the buffered frames as images which can be used as thumbnails.
Through the same API, you can show the user which portions of the video have been buffered.

## Example Usage

```rust
use iced_video_player::{VideoPlayerMessage, VideoPlayer};
use iced::{executor, Application, Command, Element, Subscription};

fn main() {
    App::run(Default::default());
}

#[derive(Debug)]
enum Message {
    VideoPlayerMessage(VideoPlayerMessage),
}

struct App {
    video: VideoPlayer,
}

impl Application for App {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        (
            App {
                video: VideoPlayer::new(&"my_video.mp4").unwrap(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::VideoPlayerMessage(msg) => self.video.update(msg).map(Message::VideoPlayerMessage),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        self.video.subscription().map(Message::VideoPlayerMessage)
    }

    fn view(&mut self) -> Element<Message> {
        self.video.view()
    }
}
```

## License

Licensed under either

- [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)
- [MIT](http://opensource.org/licenses/MIT)

at your option.
