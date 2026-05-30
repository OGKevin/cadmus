use super::{Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View};
use crate::color::TEXT_NORMAL;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::font::{DISPLAY_FONT_SIZE, DISPLAY_STYLE, Fonts, font_from_style};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;

/// Full-screen startup view shown while the app initialises.
///
/// Displays a static "Cadmus starting up…" message. Drop the view just
/// before constructing `Home` to hand off the screen.
pub struct StartupScreen {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl StartupScreen {
    pub fn new(rect: Rectangle) -> StartupScreen {
        StartupScreen {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
        }
    }
}

impl View for StartupScreen {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        true
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fb, fonts), fields(rect = ?rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        let scheme = TEXT_NORMAL;

        fb.draw_rectangle(&self.rect, scheme[0]);

        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &DISPLAY_STYLE, dpi);
        font.set_size(DISPLAY_FONT_SIZE / 2, dpi);
        let padding = font.em() as i32;
        let max_width = self.rect.width() as i32 - 3 * padding;

        let label = fl!("startup-loading");

        let mut plan = font.plan(label.as_str(), None, None);

        if plan.width > max_width {
            let scale = max_width as f32 / plan.width as f32;
            let size = (scale * (DISPLAY_FONT_SIZE / 2) as f32) as u32;
            font.set_size(size, dpi);
            plan = font.plan(label, None, None);
        }

        let dx = (self.rect.width() as i32 - plan.width) / 2;
        let dy = (self.rect.height() as i32 - font.ascender()) / 2;

        font.render(fb, scheme[1], &plan, pt!(dx, dy));

        let _ = rect;
    }

    fn might_rotate(&self) -> bool {
        false
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}
