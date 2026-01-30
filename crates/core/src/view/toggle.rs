use super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::color::{GRAY08, TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::label::Label;

use super::THICKNESS_MEDIUM;

/// A toggle component that displays two options side-by-side, separated by a vertical line.
///
/// The Toggle component provides a binary choice control where one option is highlighted
/// (inverted scheme) while the other uses a normal scheme. Tapping either label toggles
/// the state and emits a configured event.
///
/// # Visual Layout
///
/// ```text
/// ┌─────────────────────┐
/// │ Option A │ Option B │  ← enabled = true (A selected)
/// └─────────────────────┘
///     ↑           ↑
///  Inverted    Normal
///  Background  Background
/// ```
///
/// # Event Flow
///
/// 1. User taps on either label
/// 2. Label emits its configured event (bubbles to parent via bus)
/// 3. Toggle intercepts this event in its handle_event()
/// 4. Toggle updates internal state (flips enabled)
/// 5. Toggle updates both label schemes (swap inverted/normal)
/// 6. Toggle re-emits the event to continue bubbling up
///
/// # Example
///
/// ```
/// use cadmus_core::view::toggle::Toggle;
/// use cadmus_core::view::{Event, ViewId};
/// use cadmus_core::rect;
///
/// // Create a simple WiFi toggle
/// let rect = rect![10, 100, 410, 160];
/// let wifi_toggle = Toggle::new(
///     rect,
///     "On",       // First option text
///     "Off",      // Second option text
///     true,       // Initial state (On selected)
///     Event::Toggle(ViewId::SettingsMenu)
/// );
/// ```
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular bounds of the toggle
/// * `children` - Contains 3 children: [Label, Filler, Label]
/// * `enabled` - true = first option selected, false = second option selected
/// * `event` - Event to emit and intercept when toggling
pub struct Toggle {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    enabled: bool,
    event: Event,
}

impl Toggle {
    /// Creates a new Toggle component.
    ///
    /// # Arguments
    ///
    /// * `rect` - The rectangular bounds for the toggle
    /// * `text_enabled` - Text for the first option (shown inverted when enabled=true)
    /// * `text_disabled` - Text for the second option (shown inverted when enabled=false)
    /// * `enabled` - Initial state (true = first option selected)
    /// * `event` - Event to emit when toggled
    ///
    /// # Returns
    ///
    /// A new Toggle instance with two labels separated by a vertical line
    pub fn new(
        rect: Rectangle,
        text_enabled: &str,
        text_disabled: &str,
        enabled: bool,
        event: Event,
    ) -> Toggle {
        let mut children = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let separator_width = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let label_width = ((rect.width() as i32 - separator_width) / 2) as i32;

        let left_rect = rect![rect.min.x, rect.min.y, rect.min.x + label_width, rect.max.y];
        let left_scheme = if enabled {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };
        let left_label = Label::new(left_rect, text_enabled.to_string(), Align::Center)
            .scheme(left_scheme)
            .event(Some(event.clone()));
        children.push(Box::new(left_label) as Box<dyn View>);

        let separator_rect = rect![
            rect.min.x + label_width,
            rect.min.y,
            rect.min.x + label_width + separator_width,
            rect.max.y
        ];
        let separator = Filler::new(separator_rect, GRAY08);
        children.push(Box::new(separator) as Box<dyn View>);

        let right_rect = rect![
            rect.min.x + label_width + separator_width,
            rect.min.y,
            rect.max.x,
            rect.max.y
        ];
        let right_scheme = if enabled {
            TEXT_NORMAL
        } else {
            TEXT_INVERTED_HARD
        };
        let right_label = Label::new(right_rect, text_disabled.to_string(), Align::Center)
            .scheme(right_scheme)
            .event(Some(event.clone()));
        children.push(Box::new(right_label) as Box<dyn View>);

        Toggle {
            id: ID_FEEDER.next(),
            rect,
            children,
            enabled,
            event,
        }
    }

    fn update_label_schemes(&mut self, rq: &mut RenderQueue) {
        let (left_scheme, right_scheme) = if self.enabled {
            (TEXT_INVERTED_HARD, TEXT_NORMAL)
        } else {
            (TEXT_NORMAL, TEXT_INVERTED_HARD)
        };

        if let Some(left_label) = self.children[0].downcast_mut::<Label>() {
            left_label.set_scheme(left_scheme, rq);
        }

        if let Some(right_label) = self.children[2].downcast_mut::<Label>() {
            right_label.set_scheme(right_scheme, rq);
        }
    }

    #[cfg(test)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl View for Toggle {
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

            self.update_label_schemes(rq);

            bus.push_back(evt.clone());

            return true;
        }

        false
    }

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
    use crate::view::ViewId;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    #[test]
    fn test_toggle_starts_in_enabled_state() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", true, toggle_event);
        assert_eq!(toggle.is_enabled(), true);
    }

    #[test]
    fn test_toggle_starts_in_disabled_state() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", false, toggle_event);
        assert_eq!(toggle.is_enabled(), false);
    }

    #[test]
    fn test_toggle_event_intercepted_and_state_flipped() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let mut toggle = Toggle::new(rect, "On", "Off", true, toggle_event.clone());

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let handled = toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(toggle.is_enabled(), false);

        assert_eq!(bus.len(), 1);
        assert!(matches!(
            bus.pop_front(),
            Some(Event::Toggle(ViewId::SettingsMenu))
        ));

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_labels_have_correct_events_configured() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", true, toggle_event);

        let left_label = toggle.children[0].downcast_ref::<Label>().unwrap();
        assert!(left_label.text() == "On");

        let right_label = toggle.children[2].downcast_ref::<Label>().unwrap();
        assert!(right_label.text() == "Off");
    }

    #[test]
    fn test_labels_have_correct_schemes_when_enabled() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", true, toggle_event);

        let left_label = toggle.children[0].downcast_ref::<Label>().unwrap();
        assert_eq!(left_label.get_scheme(), TEXT_INVERTED_HARD);

        let right_label = toggle.children[2].downcast_ref::<Label>().unwrap();
        assert_eq!(right_label.get_scheme(), TEXT_NORMAL);
    }

    #[test]
    fn test_labels_have_correct_schemes_when_disabled() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", false, toggle_event);

        let left_label = toggle.children[0].downcast_ref::<Label>().unwrap();
        assert_eq!(left_label.get_scheme(), TEXT_NORMAL);

        let right_label = toggle.children[2].downcast_ref::<Label>().unwrap();
        assert_eq!(right_label.get_scheme(), TEXT_INVERTED_HARD);
    }

    #[test]
    fn test_filler_separator_is_present() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", true, toggle_event);

        assert!(toggle.children[1].is::<Filler>());
    }

    #[test]
    fn test_multiple_toggles_flips_state_multiple_times() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let mut toggle = Toggle::new(rect, "On", "Off", true, toggle_event.clone());

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert_eq!(toggle.is_enabled(), false);

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert_eq!(toggle.is_enabled(), true);

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);
        assert_eq!(toggle.is_enabled(), false);
    }

    #[test]
    fn test_non_toggle_events_are_ignored() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let mut toggle = Toggle::new(rect, "On", "Off", true, toggle_event);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let other_event = Event::Back;
        let handled = toggle.handle_event(&other_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert_eq!(toggle.is_enabled(), true);
        assert_eq!(bus.len(), 0);
    }

    #[test]
    fn test_event_bubbling_continues_after_toggle() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let mut toggle = Toggle::new(rect, "On", "Off", true, toggle_event.clone());

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        toggle.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(bus.len(), 1);
        let emitted_event = bus.pop_front().unwrap();
        assert!(matches!(emitted_event, Event::Toggle(ViewId::SettingsMenu)));
    }

    #[test]
    fn test_has_three_children() {
        let rect = rect![0, 0, 200, 50];
        let toggle_event = Event::Toggle(ViewId::SettingsMenu);
        let toggle = Toggle::new(rect, "On", "Off", true, toggle_event);

        assert_eq!(toggle.children.len(), 3);
        assert!(toggle.children[0].is::<Label>());
        assert!(toggle.children[1].is::<Filler>());
        assert!(toggle.children[2].is::<Label>());
    }
}
