# Iced Video Player Widget

Composable component to play videos in any Iced application built on the excellent GStreamer library.

<img src=".media/screenshot.png" width="50%" />

## Overview

In general, this supports anything that [`gstreamer/playbin`](https://gstreamer.freedesktop.org/documentation/playback/playbin.html?gi-language=c) supports.

Features:
- Load video files from any file path **or URL** (support for streaming over network).
- Video buffering when streaming on a network.
- Audio support.
- Programmatic control.
- Small (around 300 lines).

Limitations (hopefully to be fixed):
- Lazy frame syncing. Playback is usually a few frames and the decoder and audio runs ahead. Seeking is also latent.
- GStreamer is a bit annoying to set up on Windows.

This is a "composable" instead of a `iced::Widget`. This is because `Widget`s don't support subscriptions (yet?). Once Iced gets animation support (i.e. widgets scheduling a time to update), this can become a widget.

The player **does not** come with any surrounding GUI controls, but they should be quite easy to implement should you need them.

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
                video: VideoPlayer::new(&url::Url::parse("file:///C:/my_video.mp4").unwrap()).unwrap(),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::VideoPlayerMessage(msg) => self.video.update(msg),
        }
        Command::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        self.video.subscription().map(Message::VideoPlayerMessage)
    }

    fn view(&mut self) -> Element<Message> {
        self.video.frame_view().into()
    }
}
```

## Building

Follow the [GStreamer build instructions](https://github.com/sdroege/gstreamer-rs#installation). This should be able to compile on MSVC, MinGW, Linux, and MacOS.

## License

Licensed under either

- [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)
- [MIT](http://opensource.org/licenses/MIT)

at your option.
