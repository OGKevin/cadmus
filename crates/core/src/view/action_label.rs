use super::label::Label;
use super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::color::{TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::input::{DeviceEvent, FingerStatus};

/// A label that provides visual feedback when touched by inverting its colors.
///
/// The ActionLabel responds to finger down/up events by toggling an active state,
/// which switches between normal and inverted color schemes. It delegates text
/// rendering to an internal Label child. When tapped, it can emit an event through
/// the internal Label.
pub struct ActionLabel {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    active: bool,
}

impl ActionLabel {
    pub fn new(rect: Rectangle, text: String, align: Align) -> ActionLabel {
        let label = Label::new(rect, text, align);

        ActionLabel {
            id: ID_FEEDER.next(),
            rect,
            children: vec![Box::new(label)],
            active: false,
        }
    }

    /// Sets the event to be emitted when the label is tapped.
    pub fn event(mut self, event: Option<Event>) -> ActionLabel {
        if let Some(label) = self.children[0].downcast_mut::<Label>() {
            label.set_event(event);
        }
        self
    }

    /// Sets the event to be emitted when the label is tapped.
    pub fn set_event(&mut self, event: Option<Event>) {
        if let Some(label) = self.children[0].downcast_mut::<Label>() {
            label.set_event(event);
        }
    }

    /// Updates the label's text.
    pub fn update(&mut self, text: &str, rq: &mut RenderQueue) {
        if let Some(label) = self.children[0].downcast_mut::<Label>() {
            label.update(text, rq);
        }
    }

    /// Retrieves the current text value of the label.
    pub fn value(&self) -> String {
        if let Some(label) = self.children[0].downcast_ref::<Label>() {
            label.text().to_string()
        } else {
            String::new()
        }
    }

    /// Updates the label's color scheme based on the active state.
    fn update_label_scheme(&mut self, rq: &mut RenderQueue) {
        let scheme = if self.active {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };

        if let Some(label) = self.children[0].downcast_mut::<Label>() {
            label.set_scheme(scheme, rq);
        }
    }
}

impl View for ActionLabel {
    /// Handles finger down/up events to toggle active state and update label scheme.
    ///
    /// This method responds to touch input by managing the active state of the label,
    /// which controls the visual feedback through color inversion.
    ///
    /// Behavior:
    /// - **Finger Down**: If the touch position is within the label's bounds, sets the active
    ///   state to true, inverting the label's colors to provide visual feedback.
    /// - **Finger Up**: Deactivates the label and restores normal colors. This is handled
    ///   regardless of whether the finger position is within the label's bounds.
    ///
    /// Returns true if the event was handled, false otherwise.
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Device(DeviceEvent::Finger {
                status, position, ..
            }) => match status {
                FingerStatus::Down if self.rect.includes(position) => {
                    self.active = true;
                    self.update_label_scheme(rq);
                    true
                }
                FingerStatus::Up if self.active => {
                    self.active = false;
                    self.update_label_scheme(rq);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut crate::font::Fonts) {
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
    use crate::geom::Point;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_context() -> Context {
        Context::new(
            Box::new(crate::framebuffer::Pixmap::new(600, 800, 1)),
            None,
            crate::library::Library::new(
                std::path::Path::new("/tmp"),
                crate::settings::LibraryMode::Database,
            )
            .unwrap(),
            crate::settings::Settings::default(),
            crate::font::Fonts::load_from(
                std::path::Path::new(
                    &std::env::var("TEST_ROOT_DIR")
                        .expect("TEST_ROOT_DIR must be set for this test."),
                )
                .to_path_buf(),
            )
            .expect("Failed to load fonts"),
            Box::new(crate::battery::FakeBattery::new()),
            Box::new(crate::frontlight::LightLevels::default()),
            Box::new(0u16),
        )
    }

    #[test]
    fn test_new_creates_with_label_child() {
        let rect = rect![0, 0, 200, 50];
        let action_label = ActionLabel::new(rect, "Test".to_string(), Align::Right(10));

        assert_eq!(action_label.children.len(), 1);
        assert!(!action_label.active);
    }

    #[test]
    fn test_finger_down_activates() {
        let rect = rect![0, 0, 200, 50];
        let mut action_label = ActionLabel::new(rect, "Test".to_string(), Align::Right(10));
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        assert!(!action_label.active);

        let point = Point::new(100, 25);
        let event = Event::Device(DeviceEvent::Finger {
            status: FingerStatus::Down,
            position: point,
            id: 0,
            time: 0.0,
        });
        let handled = action_label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert!(action_label.active);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_finger_up_deactivates() {
        let rect = rect![0, 0, 200, 50];
        let mut action_label = ActionLabel::new(rect, "Test".to_string(), Align::Right(10));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        // Simulate active state by setting it and updating the label scheme
        action_label.active = true;
        if let Some(label) = action_label.children[0].downcast_mut::<Label>() {
            label.set_scheme(TEXT_INVERTED_HARD, &mut rq);
        }
        rq.clear();
        let mut context = create_test_context();

        let point = Point::new(100, 25);
        let event = Event::Device(DeviceEvent::Finger {
            status: FingerStatus::Up,
            position: point,
            id: 0,
            time: 0.0,
        });
        let handled = action_label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert!(!action_label.active);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_finger_down_outside_rect_ignored() {
        let rect = rect![0, 0, 200, 50];
        let mut action_label = ActionLabel::new(rect, "Test".to_string(), Align::Right(10));
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(300, 100);
        let event = Event::Device(DeviceEvent::Finger {
            status: FingerStatus::Down,
            position: point,
            id: 0,
            time: 0.0,
        });
        let handled = action_label.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert!(!action_label.active);
    }

    #[test]
    fn test_update_changes_label_text() {
        let rect = rect![0, 0, 200, 50];
        let mut action_label = ActionLabel::new(rect, "Initial".to_string(), Align::Right(10));
        let mut rq = RenderQueue::new();

        action_label.update("Updated", &mut rq);

        assert!(!rq.is_empty());
        if let Some(label) = action_label.children[0].downcast_ref::<Label>() {
            assert_eq!(label.rect(), &rect);
        }
    }

    #[test]
    fn test_event_is_emitted_on_tap() {
        let rect = rect![0, 0, 200, 50];
        let action_label =
            ActionLabel::new(rect, "Test".to_string(), Align::Right(10)).event(Some(Event::Back));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(100, 25);
        let tap_event = Event::Gesture(crate::gesture::GestureEvent::Tap(point));

        let mut boxed: Box<dyn View> = Box::new(action_label);
        crate::view::handle_event(
            boxed.as_mut(),
            &tap_event,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert_eq!(bus.len(), 1);
        assert!(matches!(bus.pop_front(), Some(Event::Back)));
    }

    #[test]
    fn test_set_event_updates_label() {
        let rect = rect![0, 0, 200, 50];
        let mut action_label = ActionLabel::new(rect, "Test".to_string(), Align::Right(10));

        action_label.set_event(Some(Event::Back));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let point = Point::new(100, 25);
        let tap_event = Event::Gesture(crate::gesture::GestureEvent::Tap(point));

        let mut boxed: Box<dyn View> = Box::new(action_label);
        crate::view::handle_event(
            boxed.as_mut(),
            &tap_event,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert_eq!(bus.len(), 1);
        assert!(matches!(bus.pop_front(), Some(Event::Back)));
    }
}
