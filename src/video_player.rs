use crate::{pipeline::VideoPrimitive, video::Video};
use gstreamer as gst;
use iced::{
    advanced::{self, graphics::core::event::Status, layout, widget, Widget},
    Element,
};
use iced_wgpu::primitive::Renderer as PrimitiveRenderer;
use log::error;
use std::{marker::PhantomData, sync::atomic::Ordering};
use std::{sync::Arc, time::Duration};

/// Video player widget which displays the current frame of a [`Video`](crate::Video).
pub struct VideoPlayer<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: PrimitiveRenderer,
{
    video: &'a Video,
    content_fit: iced::ContentFit,
    width: iced::Length,
    height: iced::Length,
    on_end_of_stream: Option<Message>,
    on_new_frame: Option<Message>,
    on_error: Option<Box<dyn Fn(&glib::Error) -> Message + 'a>>,
    _phantom: PhantomData<(Theme, Renderer)>,
}

impl<'a, Message, Theme, Renderer> VideoPlayer<'a, Message, Theme, Renderer>
where
    Renderer: PrimitiveRenderer,
{
    /// Creates a new video player widget for a given video.
    pub fn new(video: &'a Video) -> Self {
        VideoPlayer {
            video,
            content_fit: iced::ContentFit::default(),
            width: iced::Length::Shrink,
            height: iced::Length::Shrink,
            on_end_of_stream: None,
            on_new_frame: None,
            on_error: None,
            _phantom: Default::default(),
        }
    }

    /// Sets the width of the `VideoPlayer` boundaries.
    pub fn width(self, width: impl Into<iced::Length>) -> Self {
        VideoPlayer {
            width: width.into(),
            ..self
        }
    }

    /// Sets the height of the `VideoPlayer` boundaries.
    pub fn height(self, height: impl Into<iced::Length>) -> Self {
        VideoPlayer {
            height: height.into(),
            ..self
        }
    }

    /// Sets the `ContentFit` of the `VideoPlayer`.
    pub fn content_fit(self, content_fit: iced::ContentFit) -> Self {
        VideoPlayer {
            content_fit,
            ..self
        }
    }

    /// Message to send when the video reaches the end of stream (i.e., the video ends).
    pub fn on_end_of_stream(self, on_end_of_stream: Message) -> Self {
        VideoPlayer {
            on_end_of_stream: Some(on_end_of_stream),
            ..self
        }
    }

    /// Message to send when the video receives a new frame.
    pub fn on_new_frame(self, on_new_frame: Message) -> Self {
        VideoPlayer {
            on_new_frame: Some(on_new_frame),
            ..self
        }
    }

    pub fn on_error<F>(self, on_error: F) -> Self
    where
        F: 'a + Fn(&glib::Error) -> Message,
    {
        VideoPlayer {
            on_error: Some(Box::new(on_error)),
            ..self
        }
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for VideoPlayer<'a, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: PrimitiveRenderer,
{
    fn size(&self) -> iced::Size<iced::Length> {
        iced::Size {
            width: iced::Length::Shrink,
            height: iced::Length::Shrink,
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let (video_width, video_height) = self.video.size();

        // based on `Image::layout`
        let image_size = iced::Size::new(video_width as f32, video_height as f32);
        let raw_size = limits.resolve(self.width, self.height, image_size);
        let full_size = self.content_fit.fit(image_size, raw_size);
        let final_size = iced::Size {
            width: match self.width {
                iced::Length::Shrink => f32::min(raw_size.width, full_size.width),
                _ => raw_size.width,
            },
            height: match self.height {
                iced::Length::Shrink => f32::min(raw_size.height, full_size.height),
                _ => raw_size.height,
            },
        };

        layout::Node::new(final_size)
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &advanced::renderer::Style,
        layout: advanced::Layout<'_>,
        _cursor: advanced::mouse::Cursor,
        _viewport: &iced::Rectangle,
    ) {
        let inner = self.video.0.borrow_mut();

        // bounds based on `Image::draw`
        let image_size = iced::Size::new(inner.width as f32, inner.height as f32);
        let bounds = layout.bounds();
        let adjusted_fit = self.content_fit.fit(image_size, bounds.size());
        let scale = iced::Vector::new(
            adjusted_fit.width / image_size.width,
            adjusted_fit.height / image_size.height,
        );
        let final_size = image_size * scale;

        let position = match self.content_fit {
            iced::ContentFit::None => iced::Point::new(
                bounds.x + (image_size.width - adjusted_fit.width) / 2.0,
                bounds.y + (image_size.height - adjusted_fit.height) / 2.0,
            ),
            _ => iced::Point::new(
                bounds.center_x() - final_size.width / 2.0,
                bounds.center_y() - final_size.height / 2.0,
            ),
        };

        let drawing_bounds = iced::Rectangle::new(position, final_size);

        renderer.draw_primitive(
            drawing_bounds,
            VideoPrimitive::new(
                inner.id,
                Arc::clone(&inner.frame),
                (inner.width as _, inner.height as _),
                inner.upload_frame.swap(false, Ordering::SeqCst),
            ),
        );
    }

    fn on_event(
        &mut self,
        _state: &mut widget::Tree,
        event: iced::Event,
        _layout: advanced::Layout<'_>,
        _cursor: advanced::mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) -> Status {
        let mut inner = self.video.0.borrow_mut();

        if let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            if inner.restart_stream || (!inner.is_eos && !inner.paused) {
                let mut restart_stream = false;
                if inner.restart_stream {
                    restart_stream = true;
                    // Set flag to false to avoid potentially multiple seeks
                    inner.restart_stream = false;
                }
                let mut eos_pause = false;

                for msg in inner.bus.iter() {
                    match msg.view() {
                        gst::MessageView::Error(err) => {
                            error!("bus returned an error: {err}");
                            if let Some(ref on_error) = self.on_error {
                                shell.publish(on_error(&err.error()))
                            };
                        }
                        gst::MessageView::Eos(_eos) => {
                            if let Some(on_end_of_stream) = self.on_end_of_stream.clone() {
                                shell.publish(on_end_of_stream);
                            }
                            if inner.looping {
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
                    if let Err(err) = inner.restart_stream() {
                        error!("cannot restart stream (can't seek): {err:#?}")
                    }
                } else if eos_pause {
                    inner.is_eos = true;
                    inner.set_paused(true);
                }

                if inner.upload_frame.load(Ordering::SeqCst) {
                    shell.request_redraw(iced::window::RedrawRequest::NextFrame);
                    if let Some(on_new_frame) = self.on_new_frame.clone() {
                        shell.publish(on_new_frame);
                    }
                } else {
                    let redraw_interval = 1.0 / inner.framerate;
                    shell.request_redraw(iced::window::RedrawRequest::At(
                        now + Duration::from_secs_f64(redraw_interval),
                    ));
                }
            }
            Status::Captured
        } else {
            Status::Ignored
        }
    }
}

impl<'a, Message, Theme, Renderer> From<VideoPlayer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Theme: 'a,
    Renderer: 'a + PrimitiveRenderer,
{
    fn from(video_player: VideoPlayer<'a, Message, Theme, Renderer>) -> Self {
        Self::new(video_player)
    }
}
