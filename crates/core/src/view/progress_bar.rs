use super::{Bus, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, View};
use crate::color::{BLACK, PROGRESS_EMPTY, PROGRESS_FULL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{BorderSpec, CornerSpec, Rectangle, halves};
use crate::unit::scale_by_dpi;

const PROGRESS_HEIGHT: f32 = 7.0;
const BORDER_THICKNESS: f32 = 1.0;

/// A read-only horizontal progress bar.
///
/// Displays a filled track proportional to the current `percent` value (0–100).
/// Unlike [`Slider`](super::slider::Slider), this view is non-interactive and
/// has no draggable thumb — it is intended for displaying download or
/// installation progress.
pub struct ProgressBar {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    percent: u8,
}

impl ProgressBar {
    /// Creates a new progress bar at the given rect with an initial percent (0–100).
    pub fn new(rect: Rectangle, percent: u8) -> ProgressBar {
        ProgressBar {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            percent: percent.min(100),
        }
    }

    /// Updates the progress value and queues a re-render if the value changed.
    pub fn update(&mut self, percent: u8, rq: &mut RenderQueue) {
        let clamped = percent.min(100);

        if self.percent != clamped {
            self.percent = clamped;
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
    }
}

impl View for ProgressBar {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level = tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        false
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fb, _fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;
        let progress_height = scale_by_dpi(PROGRESS_HEIGHT, dpi) as i32;
        let border_thickness = scale_by_dpi(BORDER_THICKNESS, dpi) as u16;

        fb.draw_rectangle(&self.rect, WHITE);

        let (small_half, _big_half) = halves(progress_height);
        let (small_padding, big_padding) = halves(self.rect.height() as i32 - progress_height);

        let track_rect = rect![
            self.rect.min.x,
            self.rect.min.y + small_padding,
            self.rect.max.x,
            self.rect.max.y - big_padding
        ];

        let fill_x =
            self.rect.min.x + (self.rect.width() as f32 * self.percent as f32 / 100.0) as i32;

        fb.draw_rounded_rectangle_with_border(
            &track_rect,
            &CornerSpec::Uniform(small_half),
            &BorderSpec {
                thickness: border_thickness,
                color: BLACK,
            },
            &|x, _| {
                if x < fill_x {
                    PROGRESS_FULL
                } else {
                    PROGRESS_EMPTY
                }
            },
        );
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
