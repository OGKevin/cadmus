use super::super::EntryKind;
use super::super::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ID_FEEDER};
use crate::color::{TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::input::{DeviceEvent, FingerStatus};
use crate::settings::{ButtonScheme, IntermKind, Settings};
use anyhow::Error;
use core::panic;
use std::fs;
use std::path::Path;

use crate::view::EntryId;

#[derive(Debug)]
pub enum Kind {
    KeyboardLayout,
    SleepCover,
    AutoShare,
    AutoSuspend,
    AutoPowerOff,
    ButtonScheme,
    LibraryInfo(usize),
    LibraryName(usize),
    LibraryPath(usize),
    LibraryMode(usize),
    IntermissionSuspend,
    IntermissionPowerOff,
    IntermissionShare,
}

impl Kind {
    pub fn matches_interm_kind(&self, interm_kind: &IntermKind) -> bool {
        match (self, interm_kind) {
            (Kind::IntermissionSuspend, IntermKind::Suspend) => true,
            (Kind::IntermissionPowerOff, IntermKind::PowerOff) => true,
            (Kind::IntermissionShare, IntermKind::Share) => true,
            _ => false,
        }
    }
}

pub struct SettingValue {
    id: Id,
    kind: Kind,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    value: String,
    active: bool,
    entries: Vec<EntryKind>,
}

impl SettingValue {
    pub fn new(kind: Kind, rect: Rectangle, settings: &Settings) -> SettingValue {
        let (value, entries) = Self::fetch_data_for_kind(&kind, settings);

        SettingValue {
            id: ID_FEEDER.next(),
            kind,
            rect,
            children: Vec::new(),
            value,
            active: false,
            entries,
        }
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

        let schemes = vec![ButtonScheme::Natural, ButtonScheme::Inverted];
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
            let path_str = library.path.display().to_string();
            let value = format!("{}", path_str);

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
        if self.value != value {
            self.value = value;
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
    }
}

impl View for SettingValue {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Device(DeviceEvent::Finger {
                status, position, ..
            }) => match status {
                FingerStatus::Down if self.rect.includes(position) => {
                    self.active = true;

                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Fast));

                    true
                }
                FingerStatus::Up if self.active => {
                    self.active = false;

                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

                    true
                }
                _ => false,
            },
            Event::Gesture(GestureEvent::Tap(point)) if self.rect.includes(point) => {
                match self.kind {
                    Kind::LibraryInfo(index) => {
                        bus.push_back(Event::EditLibrary(index));
                    }
                    Kind::LibraryName(_) => {
                        bus.push_back(Event::Select(EntryId::EditLibraryName));
                    }
                    Kind::LibraryPath(_) => {
                        bus.push_back(Event::Select(EntryId::EditLibraryPath));
                    }
                    Kind::AutoSuspend => {
                        bus.push_back(Event::Select(EntryId::EditAutoSuspend));
                    }
                    Kind::AutoPowerOff => {
                        bus.push_back(Event::Select(EntryId::EditAutoPowerOff));
                    }
                    Kind::LibraryMode(_) => match self.entries.is_empty() {
                        true => panic!(
                            "No entries available for setting value menu of kind {:?}",
                            self.kind
                        ),
                        false => {
                            bus.push_back(Event::SubMenu(self.rect, self.entries.clone()));
                        }
                    },
                    _ => match self.entries.is_empty() {
                        true => panic!(
                            "No entries available for setting value menu of kind {:?}",
                            self.kind
                        ),
                        false => {
                            bus.push_back(Event::SubMenu(self.rect, self.entries.clone()));
                        }
                    },
                }

                true
            }
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref selected_layout)
                    if matches!(self.kind, Kind::KeyboardLayout) =>
                {
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

                    self.update(selected_layout.clone(), rq);

                    true
                }
                EntryId::ToggleSleepCover if matches!(self.kind, Kind::SleepCover) => {
                    let mut new_value = None;
                    for entry in &mut self.entries {
                        if let EntryKind::CheckBox(_, EntryId::ToggleSleepCover, ref mut checked) =
                            entry
                        {
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

                    true
                }
                EntryId::ToggleAutoShare if matches!(self.kind, Kind::AutoShare) => {
                    let mut new_value = None;
                    for entry in &mut self.entries {
                        if let EntryKind::CheckBox(_, EntryId::ToggleAutoShare, ref mut checked) =
                            entry
                        {
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

                    true
                }
                EntryId::SetButtonScheme(ref selected_scheme)
                    if matches!(self.kind, Kind::ButtonScheme) =>
                {
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

                    true
                }
                EntryId::SetLibraryMode(mode) if matches!(self.kind, Kind::LibraryMode(_)) => {
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

                    true
                }
                EntryId::SetIntermission(kind, display_kind)
                    if self.kind.matches_interm_kind(kind) =>
                {
                    for entry in &mut self.entries {
                        if let EntryKind::RadioButton(_, ref button_entry_id, ref mut selected) =
                            entry
                        {
                            *selected = matches!(
                                button_entry_id,
                                EntryId::SetIntermission(k, d) if k == kind && d == display_kind
                            );
                        }
                    }

                    self.update(display_kind.to_string(), rq);

                    true
                }
                _ => false,
            },
            Event::Submit(crate::view::ViewId::LibraryRenameInput, ref name)
                if matches!(self.kind, Kind::LibraryName(_)) =>
            {
                self.update(name.clone(), rq);
                true
            }
            Event::Submit(crate::view::ViewId::AutoSuspendInput, ref text)
                if matches!(self.kind, Kind::AutoSuspend) =>
            {
                if let Ok(value) = text.parse::<f32>() {
                    let display_value = if value == 0.0 {
                        "Never".to_string()
                    } else {
                        format!("{:.1}", value)
                    };
                    self.update(display_value, rq);
                }
                true
            }
            Event::Submit(crate::view::ViewId::AutoPowerOffInput, ref text)
                if matches!(self.kind, Kind::AutoPowerOff) =>
            {
                if let Ok(value) = text.parse::<f32>() {
                    let display_value = if value == 0.0 {
                        "Never".to_string()
                    } else {
                        format!("{:.1}", value)
                    };
                    self.update(display_value, rq);
                }
                true
            }
            Event::Submit(crate::view::ViewId::IntermissionSuspendInput, ref display_name)
                if matches!(self.kind, Kind::IntermissionSuspend) =>
            {
                self.update(display_name.clone(), rq);
                true
            }
            Event::Submit(crate::view::ViewId::IntermissionPowerOffInput, ref display_name)
                if matches!(self.kind, Kind::IntermissionPowerOff) =>
            {
                self.update(display_name.clone(), rq);
                true
            }
            Event::Submit(crate::view::ViewId::IntermissionShareInput, ref display_name)
                if matches!(self.kind, Kind::IntermissionShare) =>
            {
                self.update(display_name.clone(), rq);
                true
            }
            Event::FileChooserClosed(ref path) => match path {
                Some(ref selected_path) => match self.kind {
                    Kind::LibraryPath(_) => {
                        self.update(selected_path.display().to_string(), rq);
                        false
                    }
                    _ => false,
                },
                _ => false,
            },
            _ => false,
        }
    }

    // TODO: this can use a label as child isntea
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let scheme = if self.active {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };

        fb.draw_rectangle(&rect, scheme[0]);

        let max_width = rect.width() as i32 - padding;
        let plan = font.plan(&self.value, Some(max_width), None);
        let dy = (rect.height() as i32 - x_height) / 2;
        let dx = rect.width() as i32 - padding - plan.width;
        let pt = pt!(rect.min.x + dx, rect.max.y - dy);

        font.render(fb, scheme[1], &plan, pt);
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
        let mut context = crate::context::Context::new(
            Box::new(crate::framebuffer::Pixmap::new(600, 800, 1)),
            None,
            crate::library::Library::new(
                std::path::Path::new("/tmp"),
                crate::settings::LibraryMode::Database,
            )
            .unwrap(),
            Settings::default(),
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
        );

        let initial_suspend = suspend_value.value.clone();
        let initial_power_off = power_off_value.value.clone();
        let initial_share = share_value.value.clone();

        let test_path = PathBuf::from("/mnt/onboard/test_image.png");
        let event = Event::FileChooserClosed(Some(test_path.clone()));

        suspend_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        power_off_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);
        share_value.handle_event(&event, &hub, &mut bus, &mut rq, &mut context);

        println!("Initial suspend value: {}", initial_suspend);
        println!("After event suspend value: {}", suspend_value.value);
        println!("Initial power_off value: {}", initial_power_off);
        println!("After event power_off value: {}", power_off_value.value);
        println!("Initial share value: {}", initial_share);
        println!("After event share value: {}", share_value.value);

        assert_eq!(suspend_value.value, initial_suspend);
        assert_eq!(power_off_value.value, initial_power_off);
        assert_eq!(share_value.value, initial_share);
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
        let mut context = crate::context::Context::new(
            Box::new(crate::framebuffer::Pixmap::new(600, 800, 1)),
            None,
            crate::library::Library::new(
                std::path::Path::new("/tmp"),
                crate::settings::LibraryMode::Database,
            )
            .unwrap(),
            Settings::default(),
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
        );

        // Each value should only respond to its specific Submit event
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

        // Each value should only be updated by its matching event
        assert_eq!(suspend_value.value, "suspend_image.png");
        assert_eq!(power_off_value.value, "poweroff_image.png");
        assert_eq!(share_value.value, "share_image.png");
    }
}
