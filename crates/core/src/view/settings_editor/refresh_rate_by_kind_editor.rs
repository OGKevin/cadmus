use super::bottom_bar::BottomBarVariant;
use super::editor_utils::{
    build_bottom_separator, build_two_button_bottom_bar, calculate_dimensions,
};
use super::kinds::SettingIdentity;
use super::kinds::reader::{
    RefreshRateByKindInfo, RefreshRateByKindInverted, RefreshRateByKindRegular,
    RefreshRateInvertedSetting, RefreshRateRegularSetting,
};
use super::setting_row::SettingRow;
use super::setting_value::SettingsEvent;
use crate::color::WHITE;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::settings::{FileExtension, RefreshRatePair, Settings};
use crate::unit::scale_by_dpi;
use crate::view::SMALL_BAR_HEIGHT;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::menu::{Menu, MenuKind};
use crate::view::named_input::NamedInput;
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::{
    Bus, EntryId, Event, Hub, ID_FEEDER, Id, NotificationEvent, RenderData, RenderQueue, View,
    ViewId,
};

/// Sub-editor for the global refresh rate settings and per-file-extension overrides.
///
/// Shows two rows for the global `regular` and `inverted` values, followed by one
/// row per entry currently present in `refresh_rate.by_kind`. A "+" button in the
/// bottom bar lets the user pick an extension to add.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - Bounding rectangle
/// * `children` - Child views (background, rows, separator, bottom bar, keyboard, overlays)
/// * `add_button_rect` - Bounding rectangle of the "+" button, used to anchor the extension menu
/// * `focus` - Currently focused input view, if any
/// * `keyboard_index` - Index of the keyboard view in `children`
pub struct RefreshRateByKindEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    add_button_rect: Rectangle,
    focus: Option<ViewId>,
    keyboard_index: usize,
}

impl RefreshRateByKindEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(_hub, context, rq)))]
    pub fn new(
        rect: Rectangle,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> RefreshRateByKindEditor {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        children.push(Box::new(Filler::new(rect, WHITE)) as Box<dyn View>);

        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
            calculate_dimensions();

        children.extend(Self::build_content_rows(
            rect,
            bar_height,
            separator_thickness,
            &context.settings,
            &mut context.fonts,
        ));

        children.push(build_bottom_separator(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));
        children.push(Self::build_bottom_bar(
            rect,
            bar_height,
            separator_bottom_half,
        ));

        let keyboard = ToggleableKeyboard::new(rect, true);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        let add_button_rect = {
            let bottom_bar_min_y = rect.max.y - bar_height + separator_bottom_half;
            let mid_x = rect.min.x + (rect.max.x - rect.min.x) / 2;
            rect![mid_x, bottom_bar_min_y, rect.max.x, rect.max.y]
        };

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        RefreshRateByKindEditor {
            id,
            rect,
            children,
            add_button_rect,
            focus: None,
            keyboard_index,
        }
    }

    #[inline]
    fn build_content_rows(
        rect: Rectangle,
        bar_height: i32,
        separator_thickness: i32,
        settings: &Settings,
        fonts: &mut Fonts,
    ) -> Vec<Box<dyn View>> {
        let dpi = CURRENT_DEVICE.dpi;
        let row_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

        let content_end_y = rect.max.y - bar_height - separator_thickness;
        let mut current_y = rect.min.y;
        let mut rows: Vec<Box<dyn View>> = Vec::new();

        if current_y + row_height <= content_end_y {
            let row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            rows.push(Box::new(SettingRow::new(
                Box::new(RefreshRateRegularSetting),
                row_rect,
                settings,
                fonts,
            )));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            rows.push(Box::new(SettingRow::new(
                Box::new(RefreshRateInvertedSetting),
                row_rect,
                settings,
                fonts,
            )));
            current_y += row_height;
        }

        let mut by_kind: Vec<&str> = settings
            .reader
            .refresh_rate
            .by_kind
            .keys()
            .map(String::as_str)
            .collect();
        by_kind.sort_unstable();

        for ext_str in by_kind {
            if current_y + row_height > content_end_y {
                break;
            }

            if let Some(ext) = ext_str_to_file_extension(ext_str) {
                let row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
                rows.push(Box::new(SettingRow::new(
                    Box::new(RefreshRateByKindInfo(ext)),
                    row_rect,
                    settings,
                    fonts,
                )));
                current_y += row_height;
            }
        }

        rows
    }

    #[inline]
    fn build_bottom_bar(
        rect: Rectangle,
        bar_height: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        build_two_button_bottom_bar(
            rect,
            bar_height,
            separator_bottom_half,
            BottomBarVariant::TwoButtons {
                left_event: Event::Close(ViewId::RefreshRateByKindEditor),
                left_icon: "close",
                right_event: Event::Select(EntryId::AddRefreshRateByKind),
                right_icon: "plus",
            },
        )
    }

    /// Fixed tail: background is at index 0; separator, bottom_bar, and keyboard
    /// are the last FIXED_TAIL_LEN children appended after the content rows.
    const FIXED_TAIL_LEN: usize = 3;

    /// Rebuilds the content rows after settings change.
    fn rebuild_rows(&mut self, rq: &mut RenderQueue, context: &mut Context) {
        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
            calculate_dimensions();

        let new_rows = Self::build_content_rows(
            self.rect,
            bar_height,
            separator_thickness,
            &context.settings,
            &mut context.fonts,
        );

        let rows_end = self.children.len() - Self::FIXED_TAIL_LEN;
        self.children.drain(1..rows_end);

        for (i, row) in new_rows.into_iter().enumerate() {
            self.children.insert(1 + i, row);
        }

        self.keyboard_index = self.children.len() - 1;

        let sep_index = self.children.len() - Self::FIXED_TAIL_LEN;
        *self.children[sep_index].rect_mut() = *build_bottom_separator(
            self.rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        )
        .rect();

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    #[inline]
    fn handle_focus_event(
        &mut self,
        focus: Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.focus != focus {
            self.focus = focus;
            let keyboard = self.children[self.keyboard_index]
                .downcast_mut::<ToggleableKeyboard>()
                .expect("keyboard_index points to non-ToggleableKeyboard view");
            if focus.is_some() {
                keyboard.set_visible(true, hub, rq, context);
            } else {
                keyboard.set_visible(false, hub, rq, context);
            }
        }
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, rq, context)))]
    fn handle_add_by_kind_event(&mut self, rq: &mut RenderQueue, context: &mut Context) -> bool {
        let already: std::collections::HashSet<String> = context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .keys()
            .cloned()
            .collect();

        let entries: Vec<crate::view::EntryKind> = FileExtension::all()
            .iter()
            .filter(|ext| !already.contains(ext.as_str()))
            .map(|ext| {
                crate::view::EntryKind::Command(
                    ext.to_string(),
                    EntryId::EditRefreshRateByKind(*ext),
                )
            })
            .collect();

        if entries.is_empty() {
            return true;
        }

        let menu = Menu::new(
            self.add_button_rect,
            ViewId::SettingsValueMenu,
            MenuKind::Contextual,
            entries,
            context,
        );
        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));
        true
    }

    /// Opens a sub-editor for the given file extension's regular/inverted pair.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context), fields(ext = ?ext)))]
    fn handle_edit_by_kind_event(
        &mut self,
        ext: FileExtension,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(index);
        }

        let editor = RefreshRateKindPairEditor::new(self.rect, ext, hub, rq, context);
        self.children.push(Box::new(editor));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    /// Persists the updated refresh rate pair and rebuilds the row list.
    ///
    /// The [`RefreshRateKindPairEditor`] overlay is removed here explicitly
    /// rather than relying on a subsequent `Event::Close`. `rebuild_rows`
    /// computes the drain range from `children.len()`, so any overlay
    /// appended after the keyboard would corrupt that range if left in place.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, rq, context), fields(ext = ?ext)))]
    fn handle_update_by_kind_event(
        &mut self,
        ext: FileExtension,
        pair: &RefreshRatePair,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(index) = locate_by_id(self, ViewId::RefreshRateKindPairEditor) {
            self.children.remove(index);
        }

        context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .insert(ext.as_str().to_string(), pair.clone());
        self.rebuild_rows(rq, context);
        true
    }

    /// Removes the entry from settings and rebuilds the row list.
    ///
    /// Both the [`RefreshRateKindPairEditor`] and [`ViewId::SettingsValueMenu`]
    /// overlays are removed here explicitly rather than relying on subsequent
    /// `Event::Close` events. `rebuild_rows` computes the drain range from
    /// `children.len()`, so any overlay appended after the keyboard would
    /// corrupt that range if left in place.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, rq, context), fields(ext = ?ext)))]
    fn handle_delete_by_kind_event(
        &mut self,
        ext: FileExtension,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .remove(ext.as_str());

        if let Some(index) = locate_by_id(self, ViewId::RefreshRateKindPairEditor) {
            self.children.remove(index);
        }

        if let Some(index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(index);
        }

        self.rebuild_rows(rq, context);
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, entries, rq, context), fields(rect = ?rect)))]
    fn handle_submenu_event(
        &mut self,
        rect: Rectangle,
        entries: &[crate::view::EntryKind],
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let menu = Menu::new(
            rect,
            ViewId::SettingsValueMenu,
            MenuKind::Contextual,
            entries.to_vec(),
            context,
        );
        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));
        true
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context), fields(view_id = ?view_id)))]
    fn handle_open_named_input(
        &mut self,
        view_id: ViewId,
        label: &str,
        max_chars: usize,
        initial_text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut named_input =
            NamedInput::new(label.to_string(), view_id, view_id, max_chars, context);
        named_input.set_text(initial_text, rq, context);
        self.children.push(Box::new(named_input));
        hub.send(Event::Focus(Some(view_id))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq), fields(view_id = ?view_id)))]
    fn handle_close_event(&mut self, view_id: ViewId, hub: &Hub, rq: &mut RenderQueue) -> bool {
        match view_id {
            ViewId::SettingsValueMenu | ViewId::RefreshRateKindPairEditor => {
                if let Some(index) = locate_by_id(self, view_id) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            ViewId::RefreshRateRegularInput | ViewId::RefreshRateInvertedInput => {
                if let Some(index) = locate_by_id(self, view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            _ => false,
        }
    }
}

impl View for RefreshRateByKindEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Focus(v) => self.handle_focus_event(*v, hub, rq, context),
            Event::Select(EntryId::AddRefreshRateByKind) => {
                self.handle_add_by_kind_event(rq, context)
            }
            Event::Select(EntryId::EditRefreshRateByKind(ext)) => {
                self.handle_edit_by_kind_event(*ext, hub, rq, context)
            }
            Event::Select(EntryId::DeleteRefreshRateByKind(ext)) => {
                self.handle_delete_by_kind_event(*ext, rq, context)
            }
            Event::UpdateRefreshRateByKind(ext, pair) => {
                self.handle_update_by_kind_event(*ext, pair, rq, context)
            }
            Event::SubMenu(rect, entries) => self.handle_submenu_event(*rect, entries, rq, context),
            Event::OpenNamedInput {
                view_id,
                label,
                max_chars,
                initial_text,
            } => self.handle_open_named_input(
                *view_id,
                label,
                *max_chars,
                initial_text,
                hub,
                rq,
                context,
            ),
            Event::Close(view_id) => self.handle_close_event(*view_id, hub, rq),
            _ => {
                let _ = bus;
                false
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::RefreshRateByKindEditor)
    }
}

/// Sub-editor for editing the regular/inverted pair for a specific file extension.
struct RefreshRateKindPairEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    ext: FileExtension,
    pair: RefreshRatePair,
    focus: Option<ViewId>,
    keyboard_index: usize,
    regular_input_valid: bool,
    inverted_input_valid: bool,
}

impl RefreshRateKindPairEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(_hub, context, rq)))]
    fn new(
        rect: Rectangle,
        ext: FileExtension,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> RefreshRateKindPairEditor {
        let id = ID_FEEDER.next();
        let mut children: Vec<Box<dyn View>> = Vec::new();

        children.push(Box::new(Filler::new(rect, WHITE)) as Box<dyn View>);

        let (bar_height, separator_thickness, separator_top_half, separator_bottom_half) =
            calculate_dimensions();

        let dpi = CURRENT_DEVICE.dpi;
        let row_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let content_end_y = rect.max.y - bar_height - separator_thickness;
        let mut current_y = rect.min.y;

        let pair = context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .get(ext.as_str())
            .cloned()
            .unwrap_or(RefreshRatePair {
                regular: 0,
                inverted: 0,
            });

        if current_y + row_height <= content_end_y {
            let row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            children.push(Box::new(SettingRow::new(
                Box::new(RefreshRateByKindRegular(ext)),
                row_rect,
                &context.settings,
                &mut context.fonts,
            )));
            current_y += row_height;
        }

        if current_y + row_height <= content_end_y {
            let row_rect = rect![rect.min.x, current_y, rect.max.x, current_y + row_height];
            children.push(Box::new(SettingRow::new(
                Box::new(RefreshRateByKindInverted(ext)),
                row_rect,
                &context.settings,
                &mut context.fonts,
            )));
        }

        children.push(build_bottom_separator(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));
        children.push(build_two_button_bottom_bar(
            rect,
            bar_height,
            separator_bottom_half,
            BottomBarVariant::TwoButtons {
                left_event: Event::Close(ViewId::RefreshRateKindPairEditor),
                left_icon: "close",
                right_event: Event::Validate,
                right_icon: "check_mark-large",
            },
        ));

        let keyboard = ToggleableKeyboard::new(rect, true);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let keyboard_index = children.len() - 1;

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        RefreshRateKindPairEditor {
            id,
            rect,
            children,
            ext,
            pair,
            focus: None,
            keyboard_index,
            regular_input_valid: true,
            inverted_input_valid: true,
        }
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context)))]
    fn handle_focus_event(
        &mut self,
        focus: Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.focus != focus {
            self.focus = focus;
            let keyboard = self.children[self.keyboard_index]
                .downcast_mut::<ToggleableKeyboard>()
                .expect("keyboard_index points to non-ToggleableKeyboard view");
            if focus.is_some() {
                keyboard.set_visible(true, hub, rq, context);
            } else {
                keyboard.set_visible(false, hub, rq, context);
            }
        }
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus)))]
    fn handle_validate(&self, hub: &Hub, bus: &mut Bus) -> bool {
        if !self.regular_input_valid || !self.inverted_input_valid {
            hub.send(Event::Notification(NotificationEvent::Show(fl!(
                "notification-refresh-rate-invalid"
            ))))
            .ok();
            return true;
        }
        bus.push_back(Event::UpdateRefreshRateByKind(
            self.ext,
            Box::new(self.pair.clone()),
        ));
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, bus)))]
    fn handle_submit_regular(&mut self, text: &str, bus: &mut Bus) -> bool {
        if let Ok(v) = text.parse::<u8>() {
            self.pair.regular = v;
            self.regular_input_valid = true;
            bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::RefreshRateByKindRegular(self.ext.as_str().to_string()),
                value: v.to_string(),
            }));
        } else {
            self.regular_input_valid = false;
        }
        false
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, bus)))]
    fn handle_submit_inverted(&mut self, text: &str, bus: &mut Bus) -> bool {
        if let Ok(v) = text.parse::<u8>() {
            self.pair.inverted = v;
            self.inverted_input_valid = true;
            bus.push_back(Event::Settings(SettingsEvent::UpdateValue {
                kind: SettingIdentity::RefreshRateByKindInverted(self.ext.as_str().to_string()),
                value: v.to_string(),
            }));
        } else {
            self.inverted_input_valid = false;
        }
        false
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context), fields(view_id = ?view_id)))]
    fn handle_open_named_input(
        &mut self,
        view_id: ViewId,
        label: &str,
        max_chars: usize,
        initial_text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut named_input =
            NamedInput::new(label.to_string(), view_id, view_id, max_chars, context);
        named_input.set_text(initial_text, rq, context);
        self.children.push(Box::new(named_input));
        hub.send(Event::Focus(Some(view_id))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq), fields(view_id = ?view_id)))]
    fn handle_close_event(&mut self, view_id: ViewId, hub: &Hub, rq: &mut RenderQueue) -> bool {
        match view_id {
            ViewId::RefreshRateByKindRegularInput | ViewId::RefreshRateByKindInvertedInput => {
                if let Some(index) = locate_by_id(self, view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            _ => false,
        }
    }
}

impl View for RefreshRateKindPairEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Focus(v) => self.handle_focus_event(*v, hub, rq, context),
            Event::Validate => self.handle_validate(hub, bus),
            Event::Submit(ViewId::RefreshRateByKindRegularInput, text) => {
                self.handle_submit_regular(text, bus)
            }
            Event::Submit(ViewId::RefreshRateByKindInvertedInput, text) => {
                self.handle_submit_inverted(text, bus)
            }
            Event::OpenNamedInput {
                view_id,
                label,
                max_chars,
                initial_text,
            } => self.handle_open_named_input(
                *view_id,
                label,
                *max_chars,
                initial_text,
                hub,
                rq,
                context,
            ),
            Event::Close(view_id) => self.handle_close_event(*view_id, hub, rq),
            _ => {
                let _ = bus;
                false
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::RefreshRateKindPairEditor)
    }
}

fn ext_str_to_file_extension(s: &str) -> Option<FileExtension> {
    FileExtension::all()
        .iter()
        .find(|e| e.as_str() == s)
        .copied()
}
