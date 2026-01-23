use super::super::action_label::ActionLabel;
use super::super::EntryKind;
use super::super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::settings::{ButtonScheme, IntermKind, Settings};
use crate::view::EntryId;
use anyhow::Error;
use std::fs;
use std::path::Path;

/// Represents the type of setting value being displayed.
///
/// This enum categorizes different settings that can be configured in the application,
/// including keyboard layout, power management, button schemes, and library settings.
#[derive(Debug)]
pub enum Kind {
    /// Keyboard layout selection setting
    KeyboardLayout,
    /// Sleep cover enable/disable setting
    SleepCover,
    /// Auto-share enable/disable setting
    AutoShare,
    /// Auto-suspend timeout setting (in minutes)
    AutoSuspend,
    /// Auto power-off timeout setting (in minutes)
    AutoPowerOff,
    /// Button scheme selection (natural or inverted)
    ButtonScheme,
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
    entries: Vec<EntryKind>,
}

impl SettingValue {
    pub fn new(kind: Kind, rect: Rectangle, settings: &Settings) -> SettingValue {
        let (value, entries) = Self::fetch_data_for_kind(&kind, settings);

        let mut setting_value = SettingValue {
            id: ID_FEEDER.next(),
            kind,
            rect,
            children: vec![],
            entries,
        };

        let event = setting_value.create_tap_event();
        let action_label = ActionLabel::new(rect, value, Align::Right(10)).event(event);
        setting_value.children = vec![Box::new(action_label)];

        setting_value
    }

    fn fetch_data_for_kind(kind: &Kind, settings: &Settings) -> (String, Vec<EntryKind>) {
        match kind {
            Kind::KeyboardLayout => Self::fetch_keyboard_layout_data(settings),
            Kind::SleepCover => Self::fetch_sleep_cover_data(settings),
            Kind::AutoShare => Self::fetch_auto_share_data(settings),
            Kind::AutoSuspend => Self::fetch_auto_suspend_data(settings),
            Kind::AutoPowerOff => Self::fetch_auto_power_off_data(settings),
            Kind::ButtonScheme => Self::fetch_button_scheme_data(settings),
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
        }
    }

    fn fetch_keyboard_layout_data(settings: &Settings) -> (String, Vec<EntryKind>) {
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

        (current_layout, entries)
    }

    fn fetch_sleep_cover_data(settings: &Settings) -> (String, Vec<EntryKind>) {
        let enabled = settings.sleep_cover;
        let value = if enabled {
            "Enabled".to_string()
        } else {
            "Disabled".to_string()
        };

        let entries = vec![EntryKind::CheckBox(
            "Enable".to_string(),
            EntryId::ToggleSleepCover,
            enabled,
        )];

        (value, entries)
    }

    fn fetch_auto_share_data(settings: &Settings) -> (String, Vec<EntryKind>) {
        let enabled = settings.auto_share;
        let value = if enabled {
            "Enabled".to_string()
        } else {
            "Disabled".to_string()
        };

        let entries = vec![EntryKind::CheckBox(
            "Enable".to_string(),
            EntryId::ToggleAutoShare,
            enabled,
        )];

        (value, entries)
    }

    fn fetch_button_scheme_data(settings: &Settings) -> (String, Vec<EntryKind>) {
        let current_scheme = settings.button_scheme;
        let value = format!("{:?}", current_scheme);

        let schemes = [ButtonScheme::Natural, ButtonScheme::Inverted];
        let entries: Vec<EntryKind> = schemes
            .iter()
            .map(|scheme| {
                EntryKind::RadioButton(
                    format!("{:?}", scheme),
                    EntryId::SetButtonScheme(*scheme),
                    current_scheme == *scheme,
                )
            })
            .collect();

        (value, entries)
    }

    fn fetch_auto_suspend_data(settings: &Settings) -> (String, Vec<EntryKind>) {
        let value = if settings.auto_suspend == 0.0 {
            "Never".to_string()
        } else {
            format!("{:.1}", settings.auto_suspend)
        };

        (value, vec![])
    }

    fn fetch_auto_power_off_data(settings: &Settings) -> (String, Vec<EntryKind>) {
        let value = if settings.auto_power_off == 0.0 {
            "Never".to_string()
        } else {
            format!("{:.1}", settings.auto_power_off)
        };

        (value, vec![])
    }

    fn fetch_library_info_data(index: usize, settings: &Settings) -> (String, Vec<EntryKind>) {
        if let Some(library) = settings.libraries.get(index) {
            let value = library.path.display().to_string();

            (value, vec![])
        } else {
            ("Unknown".to_string(), vec![])
        }
    }

    fn fetch_library_name_data(index: usize, settings: &Settings) -> (String, Vec<EntryKind>) {
        if let Some(library) = settings.libraries.get(index) {
            (library.name.clone(), vec![])
        } else {
            ("Unknown".to_string(), vec![])
        }
    }

    fn fetch_library_path_data(index: usize, settings: &Settings) -> (String, Vec<EntryKind>) {
        if let Some(library) = settings.libraries.get(index) {
            (library.path.display().to_string(), vec![])
        } else {
            ("Unknown".to_string(), vec![])
        }
    }

    fn fetch_library_mode_data(index: usize, settings: &Settings) -> (String, Vec<EntryKind>) {
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
        (mode.to_string(), entries)
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
        kind: crate::settings::IntermKind,
        settings: &Settings,
    ) -> (String, Vec<EntryKind>) {
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

        (value, entries)
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

    fn create_tap_event(&self) -> Option<Event> {
        match self.kind {
            Kind::LibraryInfo(index) => Some(Event::EditLibrary(index)),
            Kind::LibraryName(_) => Some(Event::Select(EntryId::EditLibraryName)),
            Kind::LibraryPath(_) => Some(Event::Select(EntryId::EditLibraryPath)),
            Kind::AutoSuspend => Some(Event::Select(EntryId::EditAutoSuspend)),
            Kind::AutoPowerOff => Some(Event::Select(EntryId::EditAutoPowerOff)),
            _ if !self.entries.is_empty() => Some(Event::SubMenu(self.rect, self.entries.clone())),
            _ => None,
        }
    }

    fn handle_set_keyboard_layout(&mut self, selected_layout: &str, rq: &mut RenderQueue) -> bool {
        if !matches!(self.kind, Kind::KeyboardLayout) {
            return false;
        }

        for entry in &mut self.entries {
            if let EntryKind::RadioButton(
                _,
                EntryId::SetKeyboardLayout(ref layout),
                ref mut selected,
            ) = entry
            {
                *selected = layout == selected_layout
            }
        }

        self.update(selected_layout.to_string(), rq);

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_toggle_sleep_cover(&mut self, rq: &mut RenderQueue) -> bool {
        if !matches!(self.kind, Kind::SleepCover) {
            return false;
        }

        let mut new_value = None;
        for entry in &mut self.entries {
            if let EntryKind::CheckBox(_, EntryId::ToggleSleepCover, ref mut checked) = entry {
                *checked = !*checked;
                new_value = Some(if *checked {
                    "Enabled".to_string()
                } else {
                    "Disabled".to_string()
                });
            }
        }

        if let Some(value) = new_value {
            self.update(value, rq);
        }

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_toggle_auto_share(&mut self, rq: &mut RenderQueue) -> bool {
        if !matches!(self.kind, Kind::AutoShare) {
            return false;
        }

        let mut new_value = None;
        for entry in &mut self.entries {
            if let EntryKind::CheckBox(_, EntryId::ToggleAutoShare, ref mut checked) = entry {
                *checked = !*checked;
                new_value = Some(if *checked {
                    "Enabled".to_string()
                } else {
                    "Disabled".to_string()
                });
            }
        }

        if let Some(value) = new_value {
            self.update(value, rq);
        }

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_set_button_scheme(
        &mut self,
        selected_scheme: &ButtonScheme,
        rq: &mut RenderQueue,
    ) -> bool {
        if !matches!(self.kind, Kind::ButtonScheme) {
            return false;
        }

        for entry in &mut self.entries {
            if let EntryKind::RadioButton(
                _,
                EntryId::SetButtonScheme(ref scheme),
                ref mut selected,
            ) = entry
            {
                *selected = scheme == selected_scheme
            }
        }

        self.update(format!("{:?}", selected_scheme), rq);

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_set_library_mode(
        &mut self,
        mode: &crate::settings::LibraryMode,
        rq: &mut RenderQueue,
    ) -> bool {
        if !matches!(self.kind, Kind::LibraryMode(_)) {
            return false;
        }

        for entry in &mut self.entries {
            if let EntryKind::RadioButton(
                _,
                EntryId::SetLibraryMode(ref entry_mode),
                ref mut selected,
            ) = entry
            {
                *selected = entry_mode == mode
            }
        }

        self.update(format!("{:?}", mode), rq);

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_set_intermission(
        &mut self,
        kind: &IntermKind,
        display_kind: &crate::settings::IntermissionDisplay,
        rq: &mut RenderQueue,
    ) -> bool {
        if !self.kind.matches_interm_kind(kind) {
            return false;
        }

        for entry in &mut self.entries {
            if let EntryKind::RadioButton(_, ref button_entry_id, ref mut selected) = entry {
                *selected = matches!(
                    button_entry_id,
                    EntryId::SetIntermission(k, d) if k == kind && d == display_kind
                );
            }
        }

        self.update(display_kind.to_string(), rq);

        let event = self.create_tap_event();
        if let Some(action_label) = self.children[0].downcast_mut::<ActionLabel>() {
            action_label.set_event(event);
        }

        true
    }

    fn handle_submit_library_name(&mut self, name: &str, rq: &mut RenderQueue) -> bool {
        if matches!(self.kind, Kind::LibraryName(_)) {
            self.update(name.to_string(), rq);
            true
        } else {
            false
        }
    }

    fn handle_submit_auto_suspend(&mut self, text: &str, rq: &mut RenderQueue) -> bool {
        if !matches!(self.kind, Kind::AutoSuspend) {
            return false;
        }

        if let Ok(value) = text.parse::<f32>() {
            let display_value = if value.max(0.0) == 0.0 {
                "Never".to_string()
            } else {
                format!("{:.1}", value)
            };
            self.update(display_value, rq);
        }
        true
    }

    fn handle_submit_auto_power_off(&mut self, text: &str, rq: &mut RenderQueue) -> bool {
        if !matches!(self.kind, Kind::AutoPowerOff) {
            return false;
        }

        if let Ok(value) = text.parse::<f32>() {
            let display_value = if value.max(0.0) == 0.0 {
                "Never".to_string()
            } else {
                format!("{:.1}", value)
            };
            self.update(display_value, rq);
        }
        true
    }

    fn handle_submit_intermission(&mut self, display_name: &str, rq: &mut RenderQueue) -> bool {
        match self.kind {
            Kind::IntermissionSuspend | Kind::IntermissionPowerOff | Kind::IntermissionShare => {
                self.update(display_name.to_string(), rq);
                true
            }
            _ => false,
        }
    }

    fn handle_file_chooser_closed(
        &mut self,
        path: &Option<std::path::PathBuf>,
        rq: &mut RenderQueue,
    ) -> bool {
        if let Some(ref selected_path) = *path {
            if matches!(self.kind, Kind::LibraryPath(_)) {
                self.update(selected_path.display().to_string(), rq);
                return false;
            }
        }
        false
    }
}

impl View for SettingValue {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref selected_layout) => {
                    self.handle_set_keyboard_layout(selected_layout, rq)
                }
                EntryId::ToggleSleepCover => self.handle_toggle_sleep_cover(rq),
                EntryId::ToggleAutoShare => self.handle_toggle_auto_share(rq),
                EntryId::SetButtonScheme(ref selected_scheme) => {
                    self.handle_set_button_scheme(selected_scheme, rq)
                }
                EntryId::SetLibraryMode(mode) => self.handle_set_library_mode(mode, rq),
                EntryId::SetIntermission(kind, display_kind) => {
                    self.handle_set_intermission(kind, display_kind, rq)
                }
                _ => false,
            },
            Event::Submit(crate::view::ViewId::LibraryRenameInput, ref name) => {
                self.handle_submit_library_name(name, rq)
            }
            Event::Submit(crate::view::ViewId::AutoSuspendInput, ref text) => {
                self.handle_submit_auto_suspend(text, rq)
            }
            Event::Submit(crate::view::ViewId::AutoPowerOffInput, ref text) => {
                self.handle_submit_auto_power_off(text, rq)
            }
            Event::Submit(crate::view::ViewId::IntermissionSuspendInput, ref display_name) => {
                matches!(self.kind, Kind::IntermissionSuspend)
                    && self.handle_submit_intermission(display_name, rq)
            }
            Event::Submit(crate::view::ViewId::IntermissionPowerOffInput, ref display_name) => {
                matches!(self.kind, Kind::IntermissionPowerOff)
                    && self.handle_submit_intermission(display_name, rq)
            }
            Event::Submit(crate::view::ViewId::IntermissionShareInput, ref display_name) => {
                matches!(self.kind, Kind::IntermissionShare)
                    && self.handle_submit_intermission(display_name, rq)
            }
            Event::FileChooserClosed(ref path) => self.handle_file_chooser_closed(path, rq),
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
    use crate::gesture::GestureEvent;
    use crate::settings::Settings;
    use crate::view::{RenderQueue, ViewId};
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    #[test]
    fn test_file_chooser_closed_updates_all_intermission_values() {
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value = SettingValue::new(Kind::IntermissionSuspend, rect, &settings);
        let mut power_off_value = SettingValue::new(Kind::IntermissionPowerOff, rect, &settings);
        let mut share_value = SettingValue::new(Kind::IntermissionShare, rect, &settings);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

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
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut suspend_value = SettingValue::new(Kind::IntermissionSuspend, rect, &settings);
        let mut power_off_value = SettingValue::new(Kind::IntermissionPowerOff, rect, &settings);
        let mut share_value = SettingValue::new(Kind::IntermissionShare, rect, &settings);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let suspend_event = Event::Submit(
            ViewId::IntermissionSuspendInput,
            "suspend_image.png".to_string(),
        );
        let power_off_event = Event::Submit(
            ViewId::IntermissionPowerOffInput,
            "poweroff_image.png".to_string(),
        );
        let share_event = Event::Submit(
            ViewId::IntermissionShareInput,
            "share_image.png".to_string(),
        );

        suspend_value.handle_event(&suspend_event, &hub, &mut bus, &mut rq, &mut context);
        suspend_value.handle_event(&power_off_event, &hub, &mut bus, &mut rq, &mut context);
        suspend_value.handle_event(&share_event, &hub, &mut bus, &mut rq, &mut context);

        power_off_value.handle_event(&suspend_event, &hub, &mut bus, &mut rq, &mut context);
        power_off_value.handle_event(&power_off_event, &hub, &mut bus, &mut rq, &mut context);
        power_off_value.handle_event(&share_event, &hub, &mut bus, &mut rq, &mut context);

        share_value.handle_event(&suspend_event, &hub, &mut bus, &mut rq, &mut context);
        share_value.handle_event(&power_off_event, &hub, &mut bus, &mut rq, &mut context);
        share_value.handle_event(&share_event, &hub, &mut bus, &mut rq, &mut context);

        assert_eq!(suspend_value.value(), "suspend_image.png");
        assert_eq!(power_off_value.value(), "poweroff_image.png");
        assert_eq!(share_value.value(), "share_image.png");
    }

    #[test]
    fn test_keyboard_layout_select_updates_value() {
        let settings = Settings {
            keyboard_layout: "English".to_string(),
            ..Default::default()
        };
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::KeyboardLayout, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Select(EntryId::SetKeyboardLayout("French".to_string()));
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "French");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_sleep_cover_toggle_updates_value() {
        let settings = Settings {
            sleep_cover: false,
            ..Default::default()
        };
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::SleepCover, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        assert_eq!(value.value(), "Disabled");

        let event = Event::Select(EntryId::ToggleSleepCover);
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Enabled");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_share_toggle_updates_value() {
        let settings = Settings {
            auto_share: false,
            ..Default::default()
        };
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoShare, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        assert_eq!(value.value(), "Disabled");

        let event = Event::Select(EntryId::ToggleAutoShare);
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Enabled");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_button_scheme_select_updates_value() {
        let settings = Settings {
            button_scheme: ButtonScheme::Natural,
            ..Default::default()
        };
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::ButtonScheme, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Select(EntryId::SetButtonScheme(ButtonScheme::Inverted));
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Inverted");
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

        let mut value = SettingValue::new(Kind::LibraryMode(0), rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        assert_eq!(value.value(), "Filesystem");

        let event = Event::Select(EntryId::SetLibraryMode(LibraryMode::Database));
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Database");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_suspend_submit_updates_value() {
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoSuspend, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Submit(ViewId::AutoSuspendInput, "15.0".to_string());
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "15.0");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_auto_power_off_submit_updates_value() {
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoPowerOff, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Submit(ViewId::AutoPowerOffInput, "7.0".to_string());
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
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

        let mut value = SettingValue::new(Kind::LibraryName(0), rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Submit(ViewId::LibraryRenameInput, "New Name".to_string());
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
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

        let mut value = SettingValue::new(Kind::LibraryPath(0), rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let new_path = PathBuf::from("/mnt/onboard/new_library");
        let event = Event::FileChooserClosed(Some(new_path.clone()));
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(!handled);
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

        let value = SettingValue::new(Kind::LibraryInfo(0), rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

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
    fn test_handle_submit_auto_suspend_negative_value() {
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoSuspend, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Submit(ViewId::AutoSuspendInput, "-5.0".to_string());
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Never");
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_handle_submit_auto_power_off_negative_value() {
        let settings = Settings::default();
        let rect = rect![0, 0, 200, 50];

        let mut value = SettingValue::new(Kind::AutoPowerOff, rect, &settings);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let event = Event::Submit(ViewId::AutoPowerOffInput, "-5.0".to_string());
        let handled = value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(value.value(), "Never");
        assert!(!rq.is_empty());
    }
}
