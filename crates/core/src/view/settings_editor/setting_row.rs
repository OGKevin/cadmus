use super::super::Align;
use super::super::label::Label;
use super::super::{Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View};
use super::kinds::{SettingIdentity, SettingKind};
use super::setting_value::SettingValue;
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::settings::Settings;

/// A row in the settings UI that displays a setting label and its corresponding value.
///
/// `SettingRow` is a composite view that contains two child views:
/// - A `Label` displaying the human-readable name of the setting
/// - A `SettingValue` displaying the current value and allowing modifications
///
/// The row is completely driven by a [`SettingKind`] implementation — no match arms
/// are needed here.
pub struct SettingRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    /// Kept for the `UpdateLibrary` special case that relabels library rows.
    identity: SettingIdentity,
}

impl SettingRow {
    pub fn new(
        kind: impl SettingKind + 'static,
        rect: Rectangle,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> SettingRow {
        let mut children = Vec::new();

        let half_width = rect.width() as i32 / 2;
        let label_rect = rect![rect.min.x, rect.min.y, rect.min.x + half_width, rect.max.y];
        let value_rect = rect![rect.min.x + half_width, rect.min.y, rect.max.x, rect.max.y];
        let hold_event = kind.hold_event(rect);

        let label_text = kind.label(settings);
        let identity = kind.identity();

        let label =
            Label::new(label_rect, label_text, Align::Left(50)).hold_event(hold_event.clone());
        children.push(Box::new(label) as Box<dyn View>);

        let setting_value =
            SettingValue::new(kind, value_rect, settings, fonts).hold_event(hold_event);
        children.push(Box::new(setting_value) as Box<dyn View>);

        SettingRow {
            id: ID_FEEDER.next(),
            rect,
            children,
            identity,
        }
    }
}

impl View for SettingRow {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::UpdateLibrary(index, library) => {
                if let SettingIdentity::LibraryInfo(our_index) = self.identity {
                    if *index == our_index {
                        if let Some(name_view) = self.children.get_mut(0) {
                            if let Some(name_label) = name_view.as_any_mut().downcast_mut::<Label>()
                            {
                                name_label.update(&library.name, rq);
                                return true;
                            }
                        }
                    }
                }
                false
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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
    use crate::context::test_helpers::create_test_context;
    use crate::gesture::GestureEvent;
    use crate::settings::LibrarySettings;
    use crate::view::settings_editor::kinds::library::LibraryInfo;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    fn create_test_settings() -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        settings.libraries.push(LibrarySettings {
            name: "Test Library 0".to_string(),
            path: PathBuf::from("/tmp/lib0"),
            ..Default::default()
        });
        settings.libraries.push(LibrarySettings {
            name: "Test Library 1".to_string(),
            path: PathBuf::from("/tmp/lib1"),
            ..Default::default()
        });
        settings
    }

    #[test]
    fn test_update_library_event_updates_matching_row() {
        let mut context = create_test_context();
        let settings = create_test_settings();
        let rect = rect![0, 0, 400, 60];

        let mut row = SettingRow::new(LibraryInfo(0), rect, &settings, &mut context.fonts);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let updated_library = LibrarySettings {
            name: "Updated Library Name".to_string(),
            path: PathBuf::from("/tmp/updated"),
            ..Default::default()
        };

        let event = Event::UpdateLibrary(0, Box::new(updated_library));
        let handled = row.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event_ignores_non_matching() {
        let mut context = create_test_context();
        let settings = create_test_settings();
        let rect = rect![0, 0, 400, 60];

        let mut row = SettingRow::new(LibraryInfo(0), rect, &settings, &mut context.fonts);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let updated_library = LibrarySettings {
            name: "Updated Library 1".to_string(),
            path: PathBuf::from("/tmp/lib1_updated"),
            ..Default::default()
        };

        let event = Event::UpdateLibrary(1, Box::new(updated_library));
        let handled = row.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert!(rq.is_empty());
    }

    #[test]
    fn test_hold_finger_short_outside_label_rect_is_not_handled() {
        let mut context = create_test_context();
        let settings = create_test_settings();
        let rect = rect![0, 0, 400, 60];

        let mut row: Box<dyn View> = Box::new(SettingRow::new(
            LibraryInfo(0),
            rect,
            &settings,
            &mut context.fonts,
        ));

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        // Point outside the row entirely
        let point = crate::geom::Point::new(500, 100);
        let event = Event::Gesture(GestureEvent::HoldFingerShort(point, 0));

        crate::view::handle_event(row.as_mut(), &event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            bus.is_empty(),
            "No event should be pushed for out-of-bounds hold"
        );
    }
}
