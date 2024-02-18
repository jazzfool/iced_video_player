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
- Capture thumbnails from a set of timestamps.
- Decent performance. Skips a lot of the overhead from Iced `Image` and copies frame data directly to a WGPU texture, and renders using a custom WGPU render pipeline.
    For a very subjective reference, I can play back 1080p HEVC video with hardware decoding without hitches, in debug mode.

Limitations (hopefully to be fixed):
- GStreamer is a bit annoying to set up on Windows.

The player **does not** come with any surrounding GUI controls, but they should be quite easy to implement should you need them.
See the "minimal" example for a demonstration on how you could implement pausing, looping, and seeking.

## Example Usage

```rust
use iced_video_player::{Video, VideoPlayer};
use iced::{Sandbox, Element};

fn main() {
    App::run(Default::default());
}

struct App {
    video: Video,
}

impl Sandbox for App {
    type Message = ();

    fn new() -> Self {
        App {
            video: Video::new(&url::Url::parse("file:///C:/my_video.mp4").unwrap()).unwrap(),
        }
    }

    fn title(&self) -> String {
        String::from("Video Player")
    }

    fn update(&mut self, _message: ()) {}

    fn view(&mut self) -> Element<()> {
        VideoPlayer::new(&self.video).into()
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
