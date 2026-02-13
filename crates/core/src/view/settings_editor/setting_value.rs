use super::super::action_label::ActionLabel;
use super::super::EntryKind;
use super::super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::settings::{ButtonScheme, IntermKind, Settings};
use crate::view::toggle::Toggle;
use crate::view::{EntryId, ToggleEvent};
use anyhow::Error;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub enum ToggleSettings {
    /// Sleep cover enable/disable setting
    SleepCover,
    /// Auto-share enable/disable setting
    AutoShare,
    /// Button scheme selection (natural or inverted)
    ButtonScheme,
}

/// Represents the type of setting value being displayed.
///
/// This enum categorizes different settings that can be configured in the application,
/// including keyboard layout, power management, button schemes, and library settings.
#[derive(Debug, Clone)]
pub enum Kind {
    /// Keyboard layout selection setting
    KeyboardLayout,
    /// Auto-suspend timeout setting (in minutes)
    AutoSuspend,
    /// Auto power-off timeout setting (in minutes)
    AutoPowerOff,

    /// Generic toggle setting
    Toggle(ToggleSettings),

    /// Library info display for the library at the given index
    LibraryInfo(usize),
    /// Library name setting for the library at the given index
    LibraryName(usize),
    /// Library path setting for the library at the given index
    LibraryPath(usize),
    /// Library mode setting (database or filesystem) for the library at the given index
    LibraryMode(usize),
    /// Intermission display setting for suspend screen
    IntermissionSuspend,
    /// Intermission display setting for power-off screen
    IntermissionPowerOff,
    /// Intermission display setting for share screen
    IntermissionShare,
    /// Settings retention setting (how many old versions to keep)
    SettingsRetention,
}

impl Kind {
    pub fn matches_interm_kind(&self, interm_kind: &IntermKind) -> bool {
        matches!(
            (self, interm_kind),
            (Kind::IntermissionSuspend, IntermKind::Suspend)
                | (Kind::IntermissionPowerOff, IntermKind::PowerOff)
                | (Kind::IntermissionShare, IntermKind::Share)
        )
    }
}

/// Represents a single setting value display in the settings UI.
///
/// This struct manages the display and interaction of a setting value, including
/// the current value, available options (entries), and associated UI components.
/// It acts as a View that can be rendered and handle events related to setting changes.
#[derive(Debug)]
pub struct SettingValue {
    /// Unique identifier for this setting value view
    id: Id,
    /// The type of setting this value represents
    kind: Kind,
    /// The rectangular area occupied by this view
    rect: Rectangle,
    /// Child views, typically containing an ActionLabel for display
    children: Vec<Box<dyn View>>,
    /// Available options/entries for this setting (e.g., radio buttons, checkboxes)
    ///
    /// # Important
    /// Whenever this field is modified, the underlying ActionLabel's event must be updated
    /// by calling `create_tap_event()` and setting it via `action_label.set_event()`.
    /// This ensures the tap behavior reflects the current entries state.
    entries: Vec<EntryKind>,
}

impl SettingValue {
    pub fn new(
        kind: Kind,
        rect: Rectangle,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> SettingValue {
        let (value, entries, enabled_toggle) = Self::fetch_data_for_kind(&kind, settings);

        let mut setting_value = SettingValue {
            id: ID_FEEDER.next(),
            kind,
            rect,
            children: vec![],
            entries,
        };

        setting_value.children =
            vec![setting_value.kind_to_child_view(value, enabled_toggle, fonts)];

        setting_value
    }

    fn kind_to_child_view(
        &self,
        value: String,
        enabled_toggle: Option<bool>,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        let event = self.create_tap_event();

        match self.kind {
            Kind::Toggle(ref toggle) => match toggle {
                ToggleSettings::AutoShare => Box::new(Toggle::new(
                    self.rect,
                    "on",
                    "off",
                    enabled_toggle.expect("enabled bool should be Some for toggle settings"),
                    event.expect("Event should not be None for toggle"),
                    fonts,
                    Align::Right(10),
                )),
                ToggleSettings::ButtonScheme => Box::new(Toggle::new(
                    self.rect,
                    ButtonScheme::Natural.to_string().as_str(),
                    ButtonScheme::Inverted.to_string().as_str(),
                    enabled_toggle.expect("enabled bool should be Some for toggle settings"),
                    event.expect("Event should not be None for toggle"),
                    fonts,
                    Align::Right(10),
                )),
                ToggleSettings::SleepCover => Box::new(Toggle::new(
                    self.rect,
                    "on",
                    "off",
                    enabled_toggle.expect("enabled bool should be Some for toggle settings"),
                    event.expect("Event should not be None for toggle"),
                    fonts,
                    Align::Right(10),
                )),
            },
            _ => Box::new(ActionLabel::new(self.rect, value, Align::Right(10)).event(event)),
        }
    }

    /// Refreshes the displayed value by re-reading from context.settings.
    ///
    /// This method updates the ActionLabel text to reflect the current state of the setting
    /// in context.settings. It should be called whenever the underlying setting changes.
    pub fn refresh_from_context(&mut self, context: &Context, rq: &mut RenderQueue) {
        let (value, entries, _enabled_toggle) =
            Self::fetch_data_for_kind(&self.kind, &context.settings);
        self.entries = entries;
        let event = self.create_tap_event();

        if let Some(action_label) = self.children.get_mut(0) {
            if let Some(label) = action_label.as_any_mut().downcast_mut::<ActionLabel>() {
                label.update(&value, rq);
                label.set_event(event);
            }
        }
    }

    fn fetch_data_for_kind(
        kind: &Kind,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        match kind {
            Kind::KeyboardLayout => Self::fetch_keyboard_layout_data(settings),
            Kind::AutoSuspend => Self::fetch_auto_suspend_data(settings),
            Kind::AutoPowerOff => Self::fetch_auto_power_off_data(settings),
            Kind::LibraryInfo(index) => Self::fetch_library_info_data(*index, settings),
            Kind::LibraryName(index) => Self::fetch_library_name_data(*index, settings),
            Kind::LibraryPath(index) => Self::fetch_library_path_data(*index, settings),
            Kind::LibraryMode(index) => Self::fetch_library_mode_data(*index, settings),
            Kind::IntermissionSuspend => {
                Self::fetch_intermission_data(crate::settings::IntermKind::Suspend, settings)
            }
            Kind::IntermissionPowerOff => {
                Self::fetch_intermission_data(crate::settings::IntermKind::PowerOff, settings)
            }
            Kind::IntermissionShare => {
                Self::fetch_intermission_data(crate::settings::IntermKind::Share, settings)
            }
            Kind::SettingsRetention => Self::fetch_settings_retention_data(settings),
            Kind::Toggle(toggle) => match toggle {
                ToggleSettings::SleepCover => Self::fetch_sleep_cover_data(settings),
                ToggleSettings::AutoShare => Self::fetch_auto_share_data(settings),
                ToggleSettings::ButtonScheme => Self::fetch_button_scheme_data(settings),
            },
        }
    }

    fn fetch_keyboard_layout_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let current_layout = settings.keyboard_layout.clone();
        let available_layouts = Self::get_available_layouts().unwrap_or_default();

        let entries: Vec<EntryKind> = available_layouts
            .iter()
            .map(|layout| {
                EntryKind::RadioButton(
                    layout.clone(),
                    EntryId::SetKeyboardLayout(layout.clone()),
                    current_layout == *layout,
                )
            })
            .collect();

        (current_layout, entries, None)
    }

    fn fetch_sleep_cover_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let enabled = settings.sleep_cover;
        let value = if enabled {
            "Enabled".to_string()
        } else {
            "Disabled".to_string()
        };

        (value, vec![], Some(settings.sleep_cover))
    }

    fn fetch_auto_share_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let enabled = settings.auto_share;
        let value = if enabled {
            "Enabled".to_string()
        } else {
            "Disabled".to_string()
        };

        (value, vec![], Some(settings.auto_share))
    }

    fn fetch_button_scheme_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let current_scheme = settings.button_scheme;
        let value = format!("{:?}", current_scheme);

        (
            value,
            vec![],
            Some(settings.button_scheme == ButtonScheme::Natural),
        )
    }

    fn fetch_auto_suspend_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let value = if settings.auto_suspend == 0.0 {
            "Never".to_string()
        } else {
            format!("{:.1}", settings.auto_suspend)
        };

        (value, vec![], None)
    }

    fn fetch_auto_power_off_data(settings: &Settings) -> (String, Vec<EntryKind>, Option<bool>) {
        let value = if settings.auto_power_off == 0.0 {
            "Never".to_string()
        } else {
            format!("{:.1}", settings.auto_power_off)
        };

        (value, vec![], None)
    }

    #[inline]
    fn fetch_settings_retention_data(
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        let value = settings.settings_retention.to_string();

        (value, vec![], None)
    }

    fn fetch_library_info_data(
        index: usize,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        if let Some(library) = settings.libraries.get(index) {
            let value = library.path.display().to_string();

            (value, vec![], None)
        } else {
            ("Unknown".to_string(), vec![], None)
        }
    }

    fn fetch_library_name_data(
        index: usize,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        if let Some(library) = settings.libraries.get(index) {
            (library.name.clone(), vec![], None)
        } else {
            ("Unknown".to_string(), vec![], None)
        }
    }

    fn fetch_library_path_data(
        index: usize,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        if let Some(library) = settings.libraries.get(index) {
            (library.path.display().to_string(), vec![], None)
        } else {
            ("Unknown".to_string(), vec![], None)
        }
    }

    fn fetch_library_mode_data(
        index: usize,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        use crate::settings::LibraryMode;
        let mut mode = LibraryMode::Filesystem;

        if let Some(library) = settings.libraries.get(index) {
            mode = library.mode;
        }

        let entries = vec![
            EntryKind::RadioButton(
                LibraryMode::Database.to_string(),
                EntryId::SetLibraryMode(LibraryMode::Database),
                mode == LibraryMode::Database,
            ),
            EntryKind::RadioButton(
                LibraryMode::Filesystem.to_string(),
                EntryId::SetLibraryMode(LibraryMode::Filesystem),
                mode == LibraryMode::Filesystem,
            ),
        ];
        (mode.to_string(), entries, None)
    }

    fn get_available_layouts() -> Result<Vec<String>, Error> {
        let layouts_dir = Path::new("keyboard-layouts");
        let mut layouts = Vec::new();

        if layouts_dir.exists() {
            for entry in fs::read_dir(layouts_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let layout_name = stem
                            .chars()
                            .enumerate()
                            .map(|(i, c)| {
                                if i == 0 {
                                    c.to_uppercase().collect::<String>()
                                } else {
                                    c.to_string()
                                }
                            })
                            .collect::<String>();
                        layouts.push(layout_name);
                    }
                }
            }
        }

        layouts.sort();
        Ok(layouts)
    }

    fn fetch_intermission_data(
        kind: IntermKind,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>, Option<bool>) {
        use crate::settings::IntermissionDisplay;

        let display = &settings.intermissions[kind];

        let (value, is_logo, is_cover) = match display {
            IntermissionDisplay::Logo => ("Logo".to_string(), true, false),
            IntermissionDisplay::Cover => ("Cover".to_string(), false, true),
            IntermissionDisplay::Image(path) => {
                let display_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Custom")
                    .to_string();
                (display_name, false, false)
            }
        };

        let entries = vec![
            EntryKind::RadioButton(
                "Logo".to_string(),
                EntryId::SetIntermission(kind, IntermissionDisplay::Logo),
                is_logo,
            ),
            EntryKind::RadioButton(
                "Cover".to_string(),
                EntryId::SetIntermission(kind, IntermissionDisplay::Cover),
                is_cover,
            ),
            EntryKind::Command(
                "Custom Image...".to_string(),
                EntryId::EditIntermissionImage(kind),
            ),
        ];

        (value, entries, None)
    }

    pub fn update(&mut self, value: String, rq: &mut RenderQueue) {
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.update(&value, rq);
        }
    }

    pub fn value(&self) -> String {
        if let Some(action_label) = self.children[0].downcast_ref::<ActionLabel>() {
            action_label.value()
        } else {
            String::new()
        }
    }

    /// Generates the appropriate event to be triggered when this setting value is tapped.
    ///
    /// This method determines what event should be emitted based on the type of setting.
    /// It's used during initialization (in `new()`) and after updates (in various `handle_*` methods)
    /// to ensure the ActionLabel always has the correct tap behavior.
    ///
    /// The behavior varies by setting type:
    /// - **Direct edit settings** (LibraryInfo, LibraryName, LibraryPath, AutoSuspend, AutoPowerOff):
    ///   Return specific edit events that trigger their corresponding input dialogs.
    /// - **Settings with multiple options** (KeyboardLayout, SleepCover, AutoShare, ButtonScheme, LibraryMode, Intermission*):
    ///   Return a SubMenu event that displays all available entries as radio buttons or checkboxes.
    ///
    /// # Returns
    /// An Option containing:
    /// - `Some(Event)` - the event to emit on tap
    /// - `None`
    ///
    /// # Important
    /// **This method must be called every time `self.entries` is updated** to ensure the tap event
    /// reflects the current state of available entries.
    fn create_tap_event(&self) -> Option<Event> {
        match self.kind {
            Kind::LibraryInfo(index) => Some(Event::EditLibrary(index)),
            Kind::LibraryName(_) => Some(Event::Select(EntryId::EditLibraryName)),
            Kind::LibraryPath(_) => Some(Event::Select(EntryId::EditLibraryPath)),
            Kind::AutoSuspend => Some(Event::Select(EntryId::EditAutoSuspend)),
            Kind::AutoPowerOff => Some(Event::Select(EntryId::EditAutoPowerOff)),
            Kind::SettingsRetention => Some(Event::Select(EntryId::EditSettingsRetention)),
            Kind::Toggle(ref toggle) => {
                Some(Event::NewToggle(ToggleEvent::Setting(toggle.clone())))
            }
            _ if !self.entries.is_empty() => Some(Event::SubMenu(self.rect, self.entries.clone())),
            _ => None,
        }
    }
}

impl View for SettingValue {
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

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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
    use crate::view::RenderQueue;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    #[test]
    fn test_file_chooser_closed_updates_all_intermission_values() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value = SettingValue::new(
            Kind::IntermissionSuspend,
            rect,
            &settings,
            &mut context.fonts,
        );
        let mut power_off_value = SettingValue::new(
            Kind::IntermissionPowerOff,
            rect,
            &settings,
            &mut context.fonts,
        );
        let mut share_value =
            SettingValue::new(Kind::IntermissionShare, rect, &settings, &mut context.fonts);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_suspend = suspend_value.value().clone();
        let initial_power_off = power_off_value.value().clone();
        let initial_share = share_value.value().clone();

        let test_path = PathBuf::from("/mnt/onboard/test_image.png");
        let event = Event::FileChooserClosed(Some(test_path.clone()));

        suspend_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        power_off_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        share_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        println!("Initial suspend value: {}", initial_suspend);
        println!("After event suspend value: {}", suspend_value.value());
        println!("Initial power_off value: {}", initial_power_off);
        println!("After event power_off value: {}", power_off_value.value());
        println!("Initial share value: {}", initial_share);
        println!("After event share value: {}", share_value.value());

        assert_eq!(suspend_value.value(), initial_suspend);
        assert_eq!(power_off_value.value(), initial_power_off);
        assert_eq!(share_value.value(), initial_share);
    }

    #[test]
    fn test_intermission_values_update_via_submit_event() {
        use crate::settings::IntermKind;
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value = SettingValue::new(
            Kind::IntermissionSuspend,
            rect,
            &settings,
            &mut context.fonts,
        );
        let mut power_off_value = SettingValue::new(
            Kind::IntermissionPowerOff,
            rect,
            &settings,
            &mut context.fonts,
        );
        let mut share_value =
            SettingValue::new(Kind::IntermissionShare, rect, &settings, &mut context.fonts);

        let mut rq = RenderQueue::new();

        context.settings.intermissions[IntermKind::Suspend] =
            crate::settings::IntermissionDisplay::Image(PathBuf::from("suspend_image.png"));
        context.settings.intermissions[IntermKind::PowerOff] =
            crate::settings::IntermissionDisplay::Image(PathBuf::from("poweroff_image.png"));
        context.settings.intermissions[IntermKind::Share] =
            crate::settings::IntermissionDisplay::Image(PathBuf::from("share_image.png"));

        suspend_value.refresh_from_context(&context, &mut rq);
        power_off_value.refresh_from_context(&context, &mut rq);
        share_value.refresh_from_context(&context, &mut rq);

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

        let mut value =
            SettingValue::new(Kind::KeyboardLayout, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        context.settings.keyboard_layout = "French".to_string();
        value.refresh_from_context(&context, &mut rq);

        assert_eq!(value.value(), "French");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_library_mode_select_updates_value() {
        use crate::settings::{LibraryMode, LibrarySettings};
        let mut settings = Settings::default();
        settings.libraries.clear();
        let library = LibrarySettings {
            name: "Test Library".to_string(),
            path: PathBuf::from("/tmp"),
            mode: LibraryMode::Filesystem,
            ..Default::default()
        };
        settings.libraries.push(library);
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let mut value =
            SettingValue::new(Kind::LibraryMode(0), rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        assert_eq!(value.value(), "Filesystem");

        context.settings.libraries[0].mode = LibraryMode::Database;
        value.refresh_from_context(&context, &mut rq);

        assert_eq!(value.value(), "Database");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_suspend_submit_updates_value() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoSuspend, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        context.settings.auto_suspend = 15.0;
        value.refresh_from_context(&context, &mut rq);

        assert_eq!(value.value(), "15.0");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_power_off_submit_updates_value() {
        let mut context = create_test_context();
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoPowerOff, rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        context.settings.auto_power_off = 7.0;
        value.refresh_from_context(&context, &mut rq);

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
            mode: crate::settings::LibraryMode::Filesystem,
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let mut value =
            SettingValue::new(Kind::LibraryName(0), rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        context.settings.libraries[0].name = "New Name".to_string();
        value.refresh_from_context(&context, &mut rq);

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
            mode: crate::settings::LibraryMode::Filesystem,
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let mut value =
            SettingValue::new(Kind::LibraryPath(0), rect, &settings, &mut context.fonts);
        let mut rq = RenderQueue::new();

        let new_path = PathBuf::from("/mnt/onboard/new_library");
        context.settings.libraries[0].path = new_path.clone();
        value.refresh_from_context(&context, &mut rq);

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
            mode: crate::settings::LibraryMode::Filesystem,
            ..Default::default()
        });
        let rect = rect![0, 0, 200, 50];

        let mut context = create_test_context();
        let value = SettingValue::new(Kind::LibraryInfo(0), rect, &settings, &mut context.fonts);
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
}
