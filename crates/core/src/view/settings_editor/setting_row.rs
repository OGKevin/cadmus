use super::super::label::Label;
use super::super::Align;
use super::super::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use super::setting_value::{Kind as ValueKind, SettingValue};
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::settings::Settings;
use crate::view::settings_editor::ToggleSettings;

pub enum Kind {
    KeyboardLayout,
    SleepCover,
    AutoShare,
    AutoSuspend,
    AutoPowerOff,
    ButtonScheme,
    Library(usize),
    LibraryName(usize),
    LibraryPath(usize),
    LibraryMode(usize),
    IntermissionSuspend,
    IntermissionPowerOff,
    IntermissionShare,
}

impl Kind {
    /// Returns the human-readable label for this setting kind.
    ///
    /// # Arguments
    ///
    /// * `settings` - The current settings, used to look up dynamic labels (e.g., library names)
    ///
    /// # Returns
    ///
    /// A `String` containing the display label for this setting
    pub fn label(&self, settings: &Settings) -> String {
        match self {
            Kind::KeyboardLayout => "Keyboard Layout".to_string(),
            Kind::SleepCover => "Enable Sleep Cover".to_string(),
            Kind::AutoShare => "Enable Auto Share".to_string(),
            Kind::AutoSuspend => "Auto Suspend (minutes)".to_string(),
            Kind::AutoPowerOff => "Auto Power Off (days)".to_string(),
            Kind::ButtonScheme => "Button Scheme".to_string(),
            Kind::Library(index) => settings
                .libraries
                .get(*index)
                .map(|lib| lib.name.clone())
                .unwrap_or_else(|| "Unknown".to_string()),
            Kind::LibraryName(_) => "Name".to_string(),
            Kind::LibraryPath(_) => "Path".to_string(),
            Kind::LibraryMode(_) => "Mode".to_string(),
            Kind::IntermissionSuspend => "Suspend Screen".to_string(),
            Kind::IntermissionPowerOff => "Power Off Screen".to_string(),
            Kind::IntermissionShare => "Share Screen".to_string(),
        }
    }

    fn value_kind(&self) -> ValueKind {
        match self {
            Kind::KeyboardLayout => ValueKind::KeyboardLayout,

            // Kind::SleepCover => ValueKind::SleepCover,
            Kind::SleepCover => ValueKind::Toggle(ToggleSettings::SleepCover),
            // Kind::AutoShare => ValueKind::AutoShare,
            Kind::AutoShare => ValueKind::Toggle(ToggleSettings::AutoShare),
            Kind::AutoSuspend => ValueKind::AutoSuspend,
            Kind::AutoPowerOff => ValueKind::AutoPowerOff,
            // Kind::ButtonScheme => ValueKind::ButtonScheme,
            Kind::ButtonScheme => ValueKind::Toggle(ToggleSettings::ButtonScheme),
            Kind::Library(index) => ValueKind::LibraryInfo(*index),
            Kind::LibraryName(index) => ValueKind::LibraryName(*index),
            Kind::LibraryPath(index) => ValueKind::LibraryPath(*index),
            Kind::LibraryMode(index) => ValueKind::LibraryMode(*index),
            Kind::IntermissionSuspend => ValueKind::IntermissionSuspend,
            Kind::IntermissionPowerOff => ValueKind::IntermissionPowerOff,
            Kind::IntermissionShare => ValueKind::IntermissionShare,
        }
    }
}

/// A row in the settings UI that displays a setting label and its corresponding value.
///
/// `SettingRow` is a composite view that contains two child views:
/// - A `Label` displaying the human-readable name of the setting
/// - A `SettingValue` displaying the current value and allowing modifications
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular area occupied by this row
/// * `children` - Vector containing the label and value child views
/// * `kind` - The type of setting this row represents
pub struct SettingRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    kind: Kind,
}

impl SettingRow {
    pub fn new(kind: Kind, rect: Rectangle, settings: &Settings) -> SettingRow {
        let mut children = Vec::new();

        let half_width = rect.width() as i32 / 2;
        let label_rect = rect![rect.min.x, rect.min.y, rect.min.x + half_width, rect.max.y];
        let value_rect = rect![rect.min.x + half_width, rect.min.y, rect.max.x, rect.max.y];

        let label_text = kind.label(settings);
        let label = Label::new(label_rect, label_text, Align::Left(50));
        children.push(Box::new(label) as Box<dyn View>);

        let setting_value = SettingValue::new(kind.value_kind(), value_rect, settings);
        children.push(Box::new(setting_value) as Box<dyn View>);

        SettingRow {
            id: ID_FEEDER.next(),
            rect,
            children,
            kind,
        }
    }
}

impl View for SettingRow {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::UpdateLibrary(index, ref library) => match &self.kind {
                Kind::Library(our_index) => {
                    if index == our_index {
                        if let Some(name_view) = self.children.get_mut(0) {
                            if let Some(name_label) = name_view.as_any_mut().downcast_mut::<Label>()
                            {
                                name_label.update(&library.name, _rq);
                                return true;
                            }
                        }
                    }
                    false
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
    use crate::context::test_helpers::create_test_context;
    use crate::settings::{LibraryMode, LibrarySettings};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    fn create_test_settings() -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        settings.libraries.push(LibrarySettings {
            name: "Test Library 0".to_string(),
            path: PathBuf::from("/tmp/lib0"),
            mode: LibraryMode::Filesystem,
            ..Default::default()
        });
        settings.libraries.push(LibrarySettings {
            name: "Test Library 1".to_string(),
            path: PathBuf::from("/tmp/lib1"),
            mode: LibraryMode::Database,
            ..Default::default()
        });
        settings
    }

    #[test]
    fn test_update_library_event_updates_matching_row() {
        let settings = create_test_settings();
        let rect = rect![0, 0, 400, 60];

        let mut row = SettingRow::new(Kind::Library(0), rect, &settings);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let updated_library = LibrarySettings {
            name: "Updated Library Name".to_string(),
            path: PathBuf::from("/tmp/updated"),
            mode: LibraryMode::Database,
            ..Default::default()
        };

        let event = Event::UpdateLibrary(0, Box::new(updated_library.clone()));
        let handled = row.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event_ignores_non_matching() {
        let settings = create_test_settings();
        let rect = rect![0, 0, 400, 60];

        let mut row = SettingRow::new(Kind::Library(0), rect, &settings);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let updated_library = LibrarySettings {
            name: "Updated Library 1".to_string(),
            path: PathBuf::from("/tmp/lib1_updated"),
            mode: LibraryMode::Database,
            ..Default::default()
        };

        let event = Event::UpdateLibrary(1, Box::new(updated_library));
        let handled = row.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
        assert!(rq.is_empty());
    }
}
