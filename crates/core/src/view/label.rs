use super::{Align, Bus, Event, Hub, Id, RenderData, RenderQueue, View, ID_FEEDER};
use crate::color::{Color, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;

/// A text label widget that displays a single line of text.
///
/// `Label` is a UI component that renders text with configurable alignment and color scheme.
/// It can optionally respond to tap and hold gestures by emitting events.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular bounds of the label
/// * `children` - Child views (typically empty for labels)
/// * `text` - The text content to display
/// * `align` - Horizontal alignment of the text (left, center, or right)
/// * `scheme` - Color scheme as [background, foreground, border]
/// * `event` - Optional event to emit when the label is tapped
/// * `hold_event` - Optional event to emit when the label is held
pub struct Label {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    text: String,
    align: Align,
    scheme: [Color; 3],
    event: Option<Event>,
    hold_event: Option<Event>,
}

impl Label {
    pub fn new(rect: Rectangle, text: String, align: Align) -> Label {
        Label {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            text,
            align,
            scheme: TEXT_NORMAL,
            event: None,
            hold_event: None,
        }
    }

    /// Set the tap event for the label.
    pub fn event(mut self, event: Option<Event>) -> Label {
        self.event = event;
        self
    }

    /// Set the hold event for the label.
    pub fn hold_event(mut self, event: Option<Event>) -> Label {
        self.hold_event = event;
        self
    }

    /// Set the color scheme for the label.
    pub fn scheme(mut self, scheme: [Color; 3]) -> Label {
        self.scheme = scheme;
        self
    }

    /// Update the text content of the label.
    pub fn update(&mut self, text: &str, rq: &mut RenderQueue) {
        if self.text != text {
            self.text = text.to_string();
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
    }

    /// Update the color scheme of the label.
    pub fn set_scheme(&mut self, scheme: [Color; 3], rq: &mut RenderQueue) {
        if self.scheme != scheme {
            self.scheme = scheme;
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
    }

    /// Set the tap event for the label (mutable version).
    pub fn set_event(&mut self, event: Option<Event>) {
        self.event = event;
    }

    /// Get the current text of the label.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the current color scheme of the label.
    #[cfg(test)]
    pub fn get_scheme(&self) -> [Color; 3] {
        self.scheme
    }
}

impl View for Label {
    /// Handle events for this label.
    ///
    /// Processes tap and hold gestures that occur within the label's bounds.
    /// When a tap gesture is detected and the label has an associated event,
    /// that event is pushed to the bus and the event is marked as handled.
    /// Similarly, when a hold gesture is detected and the label has an associated
    /// hold event, that event is pushed to the bus.
    ///
    /// # ⚠️ Important Note
    ///
    /// **This label consumes all tap and hold gestures that occur within its bounds.**
    /// Even if no event is configured, the gesture will still be marked as handled and
    /// will not propagate to other views. You must explicitly set an event using
    /// `.event()` or `.hold_event()` for gestures to be processed. If you want taps
    /// to pass through to underlying views, you should not use this label or configure
    /// appropriate event handlers.
    ///
    /// # Arguments
    ///
    /// * `evt` - The event to handle
    /// * `_hub` - The event hub (unused)
    /// * `bus` - The event bus where events are pushed
    /// * `_rq` - The render queue (unused)
    /// * `_context` - The application context (unused)
    ///
    /// # Returns
    ///
    /// Returns `true` if the event was handled (consumed), `false` otherwise.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(center) => {
                if let Some(event) = self.event.clone() {
                    bus.push_back(event);
                }

                true
            }
            Event::Gesture(GestureEvent::HoldFingerShort(center, _))
                if self.rect.includes(center) =>
            {
                if let Some(event) = self.hold_event.clone() {
                    bus.push_back(event);
                }

                true
            }
            _ => false,
        }
    }

    /// Render the label to the framebuffer.
    ///
    /// Draws the label's background rectangle and renders the text content with proper
    /// alignment and vertical centering. The text is rendered using the normal font style
    /// and the foreground color from the label's color scheme.
    ///
    /// # Arguments
    ///
    /// * `fb` - The framebuffer to render to
    /// * `_rect` - The clipping region (unused)
    /// * `fonts` - The font manager for text rendering
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;

        fb.draw_rectangle(&self.rect, self.scheme[0]);

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;
        let max_width = self.rect.width() as i32 - padding;

        let plan = font.plan(&self.text, Some(max_width), None);

        let dx = self.align.offset(plan.width, self.rect.width() as i32);
        let dy = (self.rect.height() as i32 - x_height) / 2;
        let pt = pt!(self.rect.min.x + dx, self.rect.max.y - dy);

        font.render(fb, self.scheme[1], &plan, pt);
    }

    fn resize(
        &mut self,
        rect: Rectangle,
        _hub: &Hub,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) {
        if let Some(Event::ToggleNear(_, ref mut event_rect)) = self.event.as_mut() {
            *event_rect = rect;
        }
        self.rect = rect;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use crate::gesture::GestureEvent;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    #[test]
    fn test_tap_with_event_emits_and_consumes() {
        let rect = rect![0, 0, 200, 50];
        let mut label =
            Label::new(rect, "Test".to_string(), Align::Center).event(Some(Event::Back));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(100, 25);
        let event = Event::Gesture(GestureEvent::Tap(point));
        let handled = label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(bus.len(), 1);
        assert!(matches!(bus.pop_front(), Some(Event::Back)));
    }

    #[test]
    fn test_tap_without_event_does_consume() {
        let rect = rect![0, 0, 200, 50];
        let mut label = Label::new(rect, "Test".to_string(), Align::Center);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(100, 25);
        let event = Event::Gesture(GestureEvent::Tap(point));
        let handled = label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(bus.len(), 0);
    }

    #[test]
    fn test_tap_outside_rect_ignored() {
        let rect = rect![0, 0, 200, 50];
        let mut label =
            Label::new(rect, "Test".to_string(), Align::Center).event(Some(Event::Back));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(300, 100);
        let event = Event::Gesture(GestureEvent::Tap(point));
        let handled = label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert_eq!(bus.len(), 0);
    }
}
