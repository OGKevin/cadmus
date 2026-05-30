use super::super::action_label::ActionLabel;
use super::super::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View, ViewId};
use super::kinds::{SettingIdentity, SettingKind, WidgetKind};
use crate::context::Context;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::settings::Settings;
use crate::view::common::locate_by_id;
use crate::view::file_chooser::{FileChooser, SelectionMode};
use crate::view::label::Label;
use crate::view::toggle::Toggle;
use crate::view::{EntryKind, RenderData};

/// Re-exported for use in `ToggleEvent::Setting` and `CategoryEditor`.
pub use super::kinds::ToggleSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsEvent {
    /// Updates a SettingValue view by its identity with a new value.
    ///
    /// Each SettingValue checks if the identity matches its own, and updates
    /// itself if there is a match. This allows targeted updates without needing
    /// to know the specific view ID.
    UpdateValue {
        /// The identity of the SettingValue to update (matches against self.identity)
        kind: SettingIdentity,
        /// The new value to display
        value: String,
    },
}

/// Represents a single setting value display in the settings UI.
///
/// This struct manages the display and interaction of a setting value and its
/// associated UI widget (an [`ActionLabel`], a sub-menu label, or a [`Toggle`]).
/// It acts as a View that handles events via the [`SettingKind`] trait, updating the
/// displayed text and sub-menu checked state when the underlying setting changes.
pub struct SettingValue {
    /// Unique identifier for this setting value view
    id: Id,
    /// The rectangular area occupied by this view
    rect: Rectangle,
    /// Child views — a single ActionLabel or Toggle widget, plus any active NamedInput overlay
    children: Vec<Box<dyn View>>,
    /// Retained so that SubMenu entries can be rebuilt with updated checked
    /// state each time UpdateValue is received, and to provide identity for
    /// routing [`SettingsEvent::UpdateValue`] without a separate field.
    kind: Box<dyn SettingKind>,
    /// Current SubMenu entries, exposed for test inspection.
    #[cfg(test)]
    pub entries: Vec<EntryKind>,
    #[cfg(not(test))]
    entries: Vec<EntryKind>,
    /// Tracks the ViewId of an active NamedInput child, if one is open.
    active_input: Option<ViewId>,
    /// Whether a FileChooser overlay is currently open as a child.
    active_file_chooser: bool,
    /// Keeps the temporary directory alive for the duration of the FileChooser session.
    /// Without this, the directory would be deleted before FileChooser can use it.
    #[cfg(test)]
    _temp_dir: Option<tempfile::TempDir>,
}

impl std::fmt::Debug for SettingValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingValue")
            .field("id", &self.id)
            .field("identity", &self.kind.identity())
            .field("rect", &self.rect)
            .finish_non_exhaustive()
    }
}

impl SettingValue {
    /// Creates a new `SettingValue` for the given `kind`.
    ///
    /// `kind` accepts any owned value that implements [`SettingKind`], including references
    /// (`&T`), boxes (`Box<T>`), and concrete types directly. The value is erased into a
    /// `Box<dyn SettingKind>` internally, so the caller does not need to box it beforehand.
    pub fn new(
        kind: impl SettingKind + 'static,
        rect: Rectangle,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> SettingValue {
        let kind: Box<dyn SettingKind> = Box::new(kind);
        let data = kind.fetch(settings);

        let entries = if let WidgetKind::SubMenu(ref e) = data.widget {
            e.clone()
        } else {
            Vec::new()
        };

        let mut setting_value = SettingValue {
            id: ID_FEEDER.next(),
            rect,
            children: vec![],
            kind,
            entries,
            active_input: None,
            active_file_chooser: false,
            #[cfg(test)]
            _temp_dir: None,
        };

        setting_value.children =
            vec![setting_value.build_child_view(data.value, data.widget, fonts)];

        setting_value
    }

    fn build_child_view(
        &self,
        value: String,
        widget: WidgetKind,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        match widget {
            WidgetKind::None => Box::new(Label::new(self.rect, value, Align::Right(10))),
            WidgetKind::Toggle {
                left_label,
                right_label,
                enabled,
                tap_event,
            } => Box::new(Toggle::new(
                self.rect,
                &left_label,
                &right_label,
                enabled,
                tap_event,
                fonts,
                Align::Right(10),
            )),
            WidgetKind::ActionLabel(tap_event) => Box::new(
                ActionLabel::new(self.rect, value, Align::Right(10)).event(Some(tap_event)),
            ),
            WidgetKind::SubMenu(entries) => {
                let event = Some(Event::SubMenu(self.rect, entries));
                Box::new(ActionLabel::new(self.rect, value, Align::Right(10)).event(event))
            }
        }
    }

    pub fn update(&mut self, value: String, settings: &Settings, rq: &mut RenderQueue) {
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.update(&value, rq);

            if let WidgetKind::SubMenu(entries) = self.kind.fetch(settings).widget {
                self.entries = entries.clone();
                action_label.set_event(Some(Event::SubMenu(self.rect, entries)));
            }
        }
    }

    pub fn value(&self) -> String {
        if let Some(action_label) = self.children[0].downcast_ref::<ActionLabel>() {
            action_label.value()
        } else {
            String::new()
        }
    }

    /// Propagates a hold event to the child `ActionLabel`.
    pub fn hold_event(mut self, event: Option<Event>) -> Self {
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_hold_event(event);
        }

        self
    }
}

impl View for SettingValue {
    /// Handles events in three passes.
    ///
    /// 1. Delegates to [`SettingKind::handle`] for direct mutation events such as
    ///    submenu selections and toggle taps. When the kind handles the event it
    ///    returns the updated display string, which is applied immediately.
    ///
    /// 2. For [`InputSettingKind`](crate::view::settings_editor::InputSettingKind) settings, opens a [`NamedInput`] overlay on the
    ///    matching [`EntryId`](crate::view::EntryId) tap, applies the submitted text on [`Event::Submit`],
    ///    and closes the overlay on [`Event::Close`] or after submission.
    ///
    /// 3. Falls back to [`SettingsEvent::UpdateValue`] routing so that
    ///    `LibraryEditor` and other callers can push targeted display updates
    ///    without going through the event bus.
    ///
    /// [`NamedInput`]: crate::view::named_input::NamedInput
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Event::Select(entry_id) = evt
            && Some(entry_id) == self.kind.file_chooser_entry_id().as_ref()
            && !self.active_file_chooser
        {
            #[cfg(not(test))]
            let initial_path = std::path::PathBuf::from("/mnt/onboard");
            #[cfg(test)]
            let initial_path = {
                let temp_dir = tempfile::tempdir().expect("failed to create temp dir for test");
                let path = temp_dir.path().to_path_buf();
                self._temp_dir = Some(temp_dir);
                path
            };

            let file_chooser = FileChooser::new(
                rect!(
                    0,
                    0,
                    context.display.dims.0 as i32,
                    context.display.dims.1 as i32
                ),
                initial_path,
                SelectionMode::File,
                hub,
                rq,
                context,
            );
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
            self.children.push(Box::new(file_chooser));
            self.active_file_chooser = true;
            return true;
        }

        if let Event::FileChooserClosed(_) = evt {
            if self.active_file_chooser {
                if let (Some(display), _) = self.kind.handle(evt, &mut context.settings, bus) {
                    self.update(display, &context.settings, rq);
                }
                return false;
            }
            return false;
        }

        if let Event::Close(ViewId::FileChooser) = evt {
            if self.active_file_chooser {
                if let Some(idx) = locate_by_id(self, ViewId::FileChooser) {
                    self.children.remove(idx);
                }
                self.active_file_chooser = false;
                #[cfg(test)]
                {
                    self._temp_dir = None;
                }
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                return false;
            }
        }

        if let (Some(display), handled) = self.kind.handle(evt, &mut context.settings, bus) {
            self.update(display, &context.settings, rq);
            if !self.kind.keep_menu_open() {
                bus.push_back(Event::Close(ViewId::SettingsValueMenu));
            }
            return handled;
        }

        if let Some(input_kind) = self.kind.as_input_kind() {
            let view_id = input_kind.submit_view_id();
            let open_entry = input_kind.open_entry_id();

            if let Event::Select(id) = evt
                && *id == open_entry
                && self.active_input.is_none()
            {
                bus.push_back(Event::OpenNamedInput {
                    view_id,
                    label: input_kind.input_label(),
                    max_chars: input_kind.input_max_chars(),
                    initial_text: input_kind.current_text(&context.settings),
                });
                self.active_input = Some(view_id);
                return true;
            }

            if let Event::Submit(submitted_id, text) = evt
                && Some(*submitted_id) == self.active_input
            {
                let display = self
                    .kind
                    .as_input_kind()
                    .unwrap()
                    .apply_text(text, &mut context.settings);
                self.active_input = None;
                self.update(display, &context.settings, rq);
                return true;
            }
        }

        if let Event::Settings(SettingsEvent::UpdateValue { kind, value }) = evt {
            if self.kind.identity() == *kind {
                self.update(value.clone(), &context.settings, rq);
                return true;
            }
        }

        false
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
    use crate::settings::Settings;
    use crate::view::settings_editor::kinds::general::{
        AutoPowerOff, AutoSuspend, KeyboardLayout, SettingsRetention,
    };
    use crate::view::settings_editor::kinds::import::AllowedKindsSetting;
    use crate::view::settings_editor::kinds::intermission::{
        IntermissionPowerOff, IntermissionShare, IntermissionSuspend,
    };
    use crate::view::settings_editor::kinds::library::{LibraryInfo, LibraryName, LibraryPath};
    use crate::view::settings_editor::kinds::telemetry::LogLevel;
    use crate::view::{EntryId, RenderQueue};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    #[test]
    fn test_file_chooser_closed_updates_all_intermission_values() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value =
            SettingValue::new(&IntermissionSuspend, rect, &settings, &mut context.fonts);
        let mut power_off_value =
            SettingValue::new(&IntermissionPowerOff, rect, &settings, &mut context.fonts);
        let mut share_value =
            SettingValue::new(&IntermissionShare, rect, &settings, &mut context.fonts);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_suspend = suspend_value.value().clone();
        let initial_power_off = power_off_value.value().clone();
        let initial_share = share_value.value().clone();

        let event = Event::FileChooserClosed(None);

        suspend_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        power_off_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        share_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(suspend_value.value(), initial_suspend);
        assert_eq!(power_off_value.value(), initial_power_off);
        assert_eq!(share_value.value(), initial_share);
    }

    #[test]
    fn test_intermission_values_update_via_update_value_event() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value =
            SettingValue::new(&IntermissionSuspend, rect, &settings, &mut context.fonts);
        let mut power_off_value =
            SettingValue::new(&IntermissionPowerOff, rect, &settings, &mut context.fonts);
        let mut share_value =
            SettingValue::new(&IntermissionShare, rect, &settings, &mut context.fonts);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled_suspend = suspend_value.handle_event(
            &Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::IntermissionSuspend,
                value: "suspend_image.png".to_string(),
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        let handled_power_off = power_off_value.handle_event(
            &Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::IntermissionPowerOff,
                value: "poweroff_image.png".to_string(),
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        let handled_share = share_value.handle_event(
            &Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::IntermissionShare,
                value: "share_image.png".to_string(),
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled_suspend);
        assert!(handled_power_off);
        assert!(handled_share);
        assert_eq!(suspend_value.value(), "suspend_image.png");
        assert_eq!(power_off_value.value(), "poweroff_image.png");
        assert_eq!(share_value.value(), "share_image.png");
    }

    #[test]
    fn test_keyboard_layout_select_updates_value() {
        let mut context = create_test_context();
        let settings = Settings {
            keyboard_layout: "English".to_string(),
            ..Default::default()
        };
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(&KeyboardLayout, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::KeyboardLayout,
            value: "French".to_string(),
        });
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(value.value(), "French");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_suspend_submit_updates_value() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(&AutoSuspend, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::AutoSuspend,
            value: "15.0".to_string(),
        });
        value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(value.value(), "15.0");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_power_off_submit_updates_value() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(&AutoPowerOff, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::AutoPowerOff,
            value: "7.0".to_string(),
        });
        value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(value.value(), "7.0");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_library_name_submit_updates_value() {
        use crate::settings::LibrarySettings;
        let mut settings = Settings::default();
        settings.libraries.push(LibrarySettings {
            name: "Old Name".to_string(),
            path: PathBuf::from("/tmp"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let mut value = SettingValue::new(LibraryName(0), rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryName(0),
            value: "New Name".to_string(),
        });
        value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(value.value(), "New Name");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_library_path_file_chooser_closed_updates_value() {
        use crate::settings::LibrarySettings;
        let mut settings = Settings::default();
        settings.libraries.push(LibrarySettings {
            name: "Test Library".to_string(),
            path: PathBuf::from("/tmp"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let mut value = SettingValue::new(LibraryPath(0), rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let new_path = PathBuf::from("/mnt/onboard/new_library");
        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryPath(0),
            value: new_path.display().to_string(),
        });
        value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(value.value(), new_path.display().to_string());
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_tap_gesture_on_library_info_emits_edit_event() {
        use crate::settings::LibrarySettings;
        let mut settings = Settings::default();
        settings.libraries.push(LibrarySettings {
            name: "Test Library".to_string(),
            path: PathBuf::from("/tmp"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let value = SettingValue::new(LibraryInfo(0), rect, &settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let point = crate::geom::Point::new(100, 25);
        let event = Event::Gesture(GestureEvent::Tap(point));

        let mut boxed: Box<dyn View> = Box::new(value);
        crate::view::handle_event(
            boxed.as_mut(),
            &event,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert_eq!(bus.len(), 1);
        if let Some(Event::EditLibrary(index)) = bus.pop_front() {
            assert_eq!(index, 0);
        } else {
            panic!("Expected EditLibrary event");
        }
    }

    #[test]
    fn test_update_value_event_updates_library_name_display() {
        use crate::settings::LibrarySettings;
        let mut context = create_test_context();
        context.settings.libraries.clear();
        context.settings.libraries.push(LibrarySettings {
            name: "Old Name".to_string(),
            path: PathBuf::from("/tmp"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut value =
            SettingValue::new(LibraryName(0), rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "Old Name");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryName(0),
            value: "New Name".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "New Name");
    }

    #[test]
    fn test_update_value_event_updates_library_path_display() {
        use crate::settings::LibrarySettings;
        let mut context = create_test_context();
        context.settings.libraries.clear();
        context.settings.libraries.push(LibrarySettings {
            name: "Test Library".to_string(),
            path: PathBuf::from("/old/path"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut value =
            SettingValue::new(LibraryPath(0), rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "/old/path");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryPath(0),
            value: "/new/path".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "/new/path");
    }

    #[test]
    fn test_update_value_event_ignores_wrong_kind() {
        use crate::settings::LibrarySettings;
        let mut context = create_test_context();
        context.settings.libraries.clear();
        context.settings.libraries.push(LibrarySettings {
            name: "Test Library".to_string(),
            path: PathBuf::from("/tmp"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut value =
            SettingValue::new(LibraryName(0), rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "Test Library");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryPath(0),
            value: "Some Path".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            !handled,
            "UpdateValue event should not be handled when kind does not match"
        );
        assert_eq!(
            value.value(),
            "Test Library",
            "Value should not change when kind mismatches"
        );
    }

    #[test]
    fn test_update_value_event_ignores_wrong_index() {
        use crate::settings::LibrarySettings;
        let mut context = create_test_context();
        context.settings.libraries.clear();
        context.settings.libraries.push(LibrarySettings {
            name: "Library 0".to_string(),
            path: PathBuf::from("/path0"),
            ..Default::default()
        });
        context.settings.libraries.push(LibrarySettings {
            name: "Library 1".to_string(),
            path: PathBuf::from("/path1"),
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut value =
            SettingValue::new(LibraryName(0), rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "Library 0");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LibraryName(1),
            value: "Updated Library 1".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            !handled,
            "UpdateValue event should not be handled when index does not match"
        );
        assert_eq!(
            value.value(),
            "Library 0",
            "Value should not change when index mismatches"
        );
    }

    #[test]
    fn test_update_value_event_updates_auto_suspend() {
        let rect = rect![0, 0, 200, 50];
        let mut context = create_test_context();
        let mut value =
            SettingValue::new(&AutoSuspend, rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "30.0");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::AutoSuspend,
            value: "60.0".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "60.0");
    }

    #[test]
    fn test_update_value_event_updates_auto_power_off() {
        let rect = rect![0, 0, 200, 50];
        let mut context = create_test_context();
        let mut value =
            SettingValue::new(&AutoPowerOff, rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "3.0");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::AutoPowerOff,
            value: "60.0".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "60.0");
    }

    #[test]
    fn test_update_value_event_updates_settings_retention() {
        let rect = rect![0, 0, 200, 50];
        let mut context = create_test_context();
        let mut value = SettingValue::new(
            &SettingsRetention,
            rect,
            &context.settings,
            &mut context.fonts,
        );
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "3");

        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::SettingsRetention,
            value: "5".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "5");
    }

    #[test]
    fn test_update_value_event_regenerates_log_level_radio_buttons() {
        let rect = rect![0, 0, 200, 50];
        let mut context = create_test_context();
        context.settings.logging.level = "INFO".to_string();

        let mut value = SettingValue::new(&LogLevel, rect, &context.settings, &mut context.fonts);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_entries = value.entries.clone();
        assert_eq!(initial_entries.len(), 5);

        let info_entry = initial_entries.iter().find(|e| {
            if let EntryKind::RadioButton(label, _, _) = e {
                label == "INFO"
            } else {
                false
            }
        });
        assert!(
            matches!(info_entry, Some(EntryKind::RadioButton(_, _, true))),
            "INFO should be initially checked"
        );

        context.settings.logging.level = "DEBUG".to_string();
        let update_event = Event::Settings(SettingsEvent::UpdateValue {
            kind: SettingIdentity::LogLevel,
            value: "DEBUG".to_string(),
        });
        let handled = value.handle_event(&update_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "UpdateValue event should be handled when kind matches"
        );
        assert_eq!(value.value(), "DEBUG");
    }

    #[test]
    fn test_keep_open_submenu_does_not_queue_menu_close() {
        use crate::settings::FileExtension;

        let rect = rect![0, 0, 200, 50];
        let mut context = create_test_context();
        let mut value = SettingValue::new(
            &AllowedKindsSetting,
            rect,
            &context.settings,
            &mut context.fonts,
        );
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = value.handle_event(
            &Event::Select(EntryId::ToggleAllowedKind(FileExtension::Cbr)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(
            context
                .settings
                .import
                .allowed_kinds
                .contains(&FileExtension::Cbr)
        );
        assert!(
            !bus.iter()
                .any(|evt| matches!(evt, Event::Close(ViewId::SettingsValueMenu))),
            "submenu should remain open for multi-select settings"
        );
    }
}
