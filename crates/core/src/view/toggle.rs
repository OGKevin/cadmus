use super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::color::{BLACK, GRAY08, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{BorderSpec, Rectangle};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::label::Label;

use super::{THICKNESS_MEDIUM, THICKNESS_SMALL};

/// A minimal selection box indicator that renders tightly around selected label text.
///
/// This is a leaf view (no children) that draws a rounded rectangle border
/// around the actual rendered text dimensions.
struct SelectionBox {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    target_rect: Rectangle,
    text_width: i32,
    visible: bool,
}

impl SelectionBox {
    fn new(rect: Rectangle, target_rect: Rectangle, text_width: i32, visible: bool) -> Self {
        Self {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            target_rect,
            text_width,
            visible,
        }
    }

    fn set_target(&mut self, target_rect: Rectangle, text_width: i32, visible: bool) {
        self.target_rect = target_rect;
        self.text_width = text_width;
        self.visible = visible;
    }
}

impl View for SelectionBox {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
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

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts), fields(rect = ?rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        if !self.visible {
            return;
        }

        let render_rect = rect.intersection(&self.target_rect);
        if render_rect.is_none() {
            return;
        }

        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);

        let padding = font.em() as i32 / 2 - scale_by_dpi(3.0, dpi) as i32;
        let x_height = font.x_heights.0 as i32;
        let border_box_height = 3 * x_height;
        let border_box_width = self.text_width + padding;

        let x_offset = padding;
        let dy = (self.target_rect.height() as i32 - x_height) / 2;
        let y_offset = dy + x_height - 2 * x_height;
        let pt = self.target_rect.min + pt!(x_offset, y_offset);
        let border_box_rect = rect![pt, pt + pt!(border_box_width, border_box_height)];

        let border_thickness = scale_by_dpi(THICKNESS_SMALL, dpi) as u16;

        fb.draw_rectangle_outline(
            &border_box_rect,
            &BorderSpec {
                thickness: border_thickness,
                color: BLACK,
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

/// A toggle component that displays two options side-by-side, separated by a vertical line.
///
/// The Toggle component provides a binary choice control where one option is highlighted
/// with a minimal border box while the other appears without highlighting. Tapping either
/// label toggles the state and emits a configured event.
///
/// # Implementation Note
///
/// Toggle uses a child view approach for the selection box. The SelectionBox is added as
/// the 4th child and renders on top of the labels (due to z-order). When the toggle state
/// changes, the SelectionBox is updated to reposition around the selected label.
///
/// # Visual Layout
///
/// ```text
/// ┌─────────────────────────┐
/// │ ┌─────────┐ │           │
/// │ │Option A │ │ Option B  │ ← enabled = true (A selected)
/// │ └─────────┘ │           │
/// └─────────────────────────┘
///      ↑             ↑
///   Selected      Normal
///   (border)   (no border)
/// ```
///
/// # Event Flow
///
/// 1. User taps on either label
/// 2. Label emits its configured event (bubbles to parent via bus)
/// 3. Toggle intercepts this event in its handle_event()
/// 4. Toggle updates internal state (flips enabled)
/// 5. Toggle updates the SelectionBox child to reposition
/// 6. Toggle triggers a re-render
/// 7. Toggle re-emits the event to continue bubbling up
///
/// # Example
///
/// ```
/// use cadmus_core::view::toggle::Toggle;
/// use cadmus_core::view::{Align, Event, ViewId, ToggleEvent};
/// use cadmus_core::font::Fonts;
/// use cadmus_core::rect;
/// use std::env;
/// use std::path::PathBuf;
///
/// let fonts = &mut Fonts::load_from(PathBuf::from(env::var("TEST_ROOT_DIR").unwrap())).unwrap();
///
/// let rect = rect![10, 100, 410, 160];
/// let wifi_toggle = Toggle::new(
///     rect,
///     "On",       // First option text
///     "Off",      // Second option text
///     true,       // Initial state (On selected)
///     Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu)),
///     fonts,
///     Align::Right(10)
/// );
/// ```
///
/// # Alignment Behavior
///
/// The right label uses the provided alignment, while the left label remains
/// centered to avoid crowding the separator. This keeps the toggle right-aligned
/// with other setting values while maintaining consistent padding to the edge.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular bounds of the toggle
/// * `children` - Contains 4 children: [Label, Filler, Label, SelectionBox]
/// * `enabled` - true = first option selected, false = second option selected
/// * `event` - Event to emit and intercept when toggling
/// * `left_label_index` - Index of left label in children vec
/// * `right_label_index` - Index of right label in children vec
/// * `selection_box_index` - Index of selection box in children vec
pub struct Toggle {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    enabled: bool,
    event: Event,
    left_label_index: usize,
    right_label_index: usize,
    selection_box_index: usize,
    left_text_width: i32,
    right_text_width: i32,
}

impl Toggle {
    /// Creates a new Toggle component.
    ///
    /// # Arguments
    ///
    /// * `rect` - The rectangular bounds for the toggle
    /// * `text_enabled` - Text for the first option (shown with border when enabled=true)
    /// * `text_disabled` - Text for the second option (shown with border when enabled=false)
    /// * `enabled` - Initial state (true = first option selected)
    /// * `event` - Event to emit when toggled
    /// * `align` - Alignment to apply to the right label
    ///
    /// # Returns
    ///
    /// A new Toggle instance with two labels separated by a vertical line, right-aligned
    pub fn new(
        rect: Rectangle,
        text_enabled: &str,
        text_disabled: &str,
        enabled: bool,
        event: Event,
        fonts: &mut Fonts,
        align: Align,
    ) -> Toggle {
        let dpi = CURRENT_DEVICE.dpi;
        let separator_width = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let padding = font.em() as i32;
        let left_plan = font.plan(text_enabled, None, None);
        let right_plan = font.plan(text_disabled, None, None);
        let left_text_width = left_plan.width;
        let right_text_width = right_plan.width;
        let left_width = left_text_width + padding;
        let right_width = right_text_width + padding;
        let total_width = left_width + separator_width + right_width;

        let x_offset = rect.width() as i32 - total_width;

        let mut children = Vec::new();

        let left_rect = rect![
            rect.min.x + x_offset,
            rect.min.y,
            rect.min.x + x_offset + left_width,
            rect.max.y
        ];
        let left_label = Label::new(left_rect, text_enabled.to_string(), Align::Center)
            .scheme(TEXT_NORMAL)
            .event(Some(event.clone()));
        children.push(Box::new(left_label) as Box<dyn View>);
        let left_label_index = children.len() - 1;

        let separator_height = rect.height() as i32;
        let separator_padding = separator_height / 4;
        let separator_rect = rect![
            rect.min.x + x_offset + left_width,
            rect.min.y + separator_padding,
            rect.min.x + x_offset + left_width + separator_width,
            rect.max.y - separator_padding
        ];
        let separator = Filler::new(separator_rect, GRAY08);
        children.push(Box::new(separator) as Box<dyn View>);

        let right_rect = rect![
            rect.min.x + x_offset + left_width + separator_width,
            rect.min.y,
            rect.max.x,
            rect.max.y
        ];
        let right_label = Label::new(right_rect, text_disabled.to_string(), align)
            .scheme(TEXT_NORMAL)
            .event(Some(event.clone()));
        children.push(Box::new(right_label) as Box<dyn View>);
        let right_label_index = children.len() - 1;

        let selected_rect = if enabled { left_rect } else { right_rect };
        let selected_text_width = if enabled {
            left_text_width
        } else {
            right_text_width
        };
        let selection_box = SelectionBox::new(rect, selected_rect, selected_text_width, true);
        children.push(Box::new(selection_box) as Box<dyn View>);
        let selection_box_index = children.len() - 1;

        Toggle {
            id: ID_FEEDER.next(),
            rect,
            children,
            enabled,
            event,
            left_label_index,
            right_label_index,
            selection_box_index,
            left_text_width,
            right_text_width,
        }
    }

    fn request_rerender(&mut self, rq: &mut RenderQueue) {
        rq.add(crate::view::RenderData::new(
            self.id,
            self.rect,
            crate::framebuffer::UpdateMode::Gui,
        ));
    }

    fn update_selection_box(&mut self, rq: &mut RenderQueue) {
        let selected_label_index = if self.enabled {
            self.left_label_index
        } else {
            self.right_label_index
        };

        let text_width = if self.enabled {
            self.left_text_width
        } else {
            self.right_text_width
        };

        let selected_rect = *self.children[selected_label_index].rect();

        if let Some(selection_box) =
            self.children[self.selection_box_index].downcast_mut::<SelectionBox>()
        {
            selection_box.set_target(selected_rect, text_width, true);
        }
        self.request_rerender(rq);
    }

    #[cfg(test)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl View for Toggle {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, bus, rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        if std::mem::discriminant(evt) == std::mem::discriminant(&self.event) {
            self.enabled = !self.enabled;
            self.update_selection_box(rq);
            bus.push_back(evt.clone());
            return true;
        }

        false
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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
    use crate::view::{ToggleEvent, ViewId};
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    #[test]
    fn test_toggle_starts_in_enabled_state() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );
        assert!(toggle.is_enabled());
    }

    #[test]
    fn test_toggle_starts_in_disabled_state() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            false,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );
        assert!(!toggle.is_enabled());
    }

    #[test]
    fn test_toggle_event_intercepted_and_state_flipped() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let mut toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event.clone(),
            &mut context.fonts,
            Align::Center,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert!(!toggle.is_enabled());

        assert_eq!(bus.len(), 1);
        assert!(matches!(
            bus.pop_front(),
            Some(Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu)))
        ));

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_labels_have_correct_events_configured() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );

        let left_label = toggle.children[0].downcast_ref::<Label>().unwrap();
        assert!(left_label.text() == "On");

        let right_label = toggle.children[2].downcast_ref::<Label>().unwrap();
        assert!(right_label.text() == "Off");
    }

    #[test]
    fn test_labels_use_normal_scheme() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );

        let left_label = toggle.children[0].downcast_ref::<Label>().unwrap();
        assert_eq!(left_label.get_scheme(), TEXT_NORMAL);

        let right_label = toggle.children[2].downcast_ref::<Label>().unwrap();
        assert_eq!(right_label.get_scheme(), TEXT_NORMAL);
    }

    #[test]
    fn test_filler_separator_is_present() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );

        assert!(toggle.children[1].is::<Filler>());
    }

    #[test]
    fn test_multiple_toggles_flips_state_multiple_times() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let mut toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event.clone(),
            &mut context.fonts,
            Align::Center,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert!(!toggle.is_enabled());

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert!(toggle.is_enabled());

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert!(!toggle.is_enabled());
    }

    #[test]
    fn test_non_toggle_events_are_ignored() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let mut toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let other_event = Event::Back;
        let handled = toggle.handle_event(&other_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert!(toggle.is_enabled());
        assert_eq!(bus.len(), 0);
    }

    #[test]
    fn test_event_bubbling_continues_after_toggle() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let mut toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event.clone(),
            &mut context.fonts,
            Align::Center,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(bus.len(), 1);
        let emitted_event = bus.pop_front().unwrap();
        assert!(matches!(
            emitted_event,
            Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu))
        ));
    }

    #[test]
    fn test_has_four_children() {
        let mut context = create_test_context();
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::NewToggle(ToggleEvent::View(ViewId::SettingsMenu));
        let toggle = Toggle::new(
            rect,
            "On",
            "Off",
            true,
            toggle_event,
            &mut context.fonts,
            Align::Center,
        );

        assert_eq!(toggle.children.len(), 4);
        assert!(toggle.children[0].is::<Label>());
        assert!(toggle.children[1].is::<Filler>());
        assert!(toggle.children[2].is::<Label>());
        assert!(toggle.children[3].is::<SelectionBox>());
    }
}
