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
use crate::settings::ButtonScheme;
use anyhow::Error;
use std::fs;
use std::path::Path;

use crate::view::EntryId;

pub enum Kind {
    KeyboardLayout,
    SleepCover,
    AutoShare,
    ButtonScheme,
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
    pub fn new(kind: Kind, rect: Rectangle, context: &Context) -> SettingValue {
        let (value, entries) = Self::fetch_data_for_kind(&kind, context);

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

    fn fetch_data_for_kind(kind: &Kind, context: &Context) -> (String, Vec<EntryKind>) {
        match kind {
            Kind::KeyboardLayout => Self::fetch_keyboard_layout_data(context),
            Kind::SleepCover => Self::fetch_sleep_cover_data(context),
            Kind::AutoShare => Self::fetch_auto_share_data(context),
            Kind::ButtonScheme => Self::fetch_button_scheme_data(context),
        }
    }

    fn fetch_keyboard_layout_data(context: &Context) -> (String, Vec<EntryKind>) {
        let current_layout = context.settings.keyboard_layout.clone();
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

    fn fetch_sleep_cover_data(context: &Context) -> (String, Vec<EntryKind>) {
        let enabled = context.settings.sleep_cover;
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

    fn fetch_auto_share_data(context: &Context) -> (String, Vec<EntryKind>) {
        let enabled = context.settings.auto_share;
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

    fn fetch_button_scheme_data(context: &Context) -> (String, Vec<EntryKind>) {
        let current_scheme = context.settings.button_scheme;
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
                bus.push_back(Event::SubMenu(self.rect, self.entries.clone()));

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
                _ => false,
            },
            // Event::Validate => {
            //     bus.push_back(Event::Close(crate::view::ViewId::SettingsValueMenu));

            //     true
            // }
            _ => false,
        }
    }

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
