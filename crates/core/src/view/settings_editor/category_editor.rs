use crate::color::{BLACK, WHITE};
use crate::context::{Context, DICTIONARIES_DIRNAME};
use crate::device::CURRENT_DEVICE;
use crate::dictionary::MonolingualDictionaryService;
use crate::fl;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{CycleDir, Rectangle, halves};
use crate::settings::{LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::dialog::Dialog;
use crate::view::filler::Filler;
use crate::view::menu::{Menu, MenuKind};
use crate::view::toggleable_keyboard::ToggleableKeyboard;
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, ID_FEEDER, Id, NotificationEvent, RenderData, RenderQueue,
    SMALL_BAR_HEIGHT, THICKNESS_MEDIUM, View, ViewId,
};

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
use super::category::Category;
use super::library_editor::LibraryEditor;
use super::refresh_rate_by_kind_editor::RefreshRateByKindEditor;
use super::setting_row::SettingRow;
use std::path::{Path, PathBuf};
use std::thread;

/// A view for editing category-specific settings.
///
/// The `CategoryEditor` manages the UI for editing settings within a specific category
/// (e.g., Libraries, Intermissions, etc.). It displays setting rows, handles user interactions,
/// and manages child views such as keyboards and input fields.
///
/// All settings changes are applied immediately to `context.settings`, providing instant
/// feedback without requiring explicit validation.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - The rectangular area occupied by this view
/// * `children` - Child views managed by this editor. The structure is:
///   1. Content background filler (index 0)
///   2. Setting rows (indices from `first_row_index` onwards)
///   3. BottomSeparator (variable index, always present)
///   4. BottomBar (variable index, always present)
///   5. ToggleableKeyboard (at index `keyboard_index`)
///   6. Plus optional overlay views like LibraryEditor and NamedInput fields
/// * `category` - The settings category being edited
/// * `content_rect` - The rectangular area where setting rows are displayed
/// * `row_height` - The height of each setting row
/// * `focus` - Currently focused child view, if any
/// * `first_row_index` - Index in the children vector where setting rows begin (after structural elements)
/// * `separator_index` - Index of the bottom separator child view, always `first_row_index + rows_on_page`
/// * `keyboard_index` - Index of the keyboard child view, always `separator_index + 2` (sep + bar)
/// * `current_page` - The zero-based index of the currently displayed page of rows
/// * `pages_count` - Total number of pages required to display all setting rows
pub struct CategoryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    category: Category,
    content_rect: Rectangle,
    row_height: i32,
    focus: Option<ViewId>,
    first_row_index: usize,
    separator_index: usize,
    keyboard_index: usize,
    current_page: usize,
    pages_count: usize,
    dict_service: Option<MonolingualDictionaryService>,
}

impl CategoryEditor {
    pub fn new(
        rect: Rectangle,
        category: Category,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> CategoryEditor {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, separator_top_half, separator_bottom_half) = Self::calculate_dimensions();

        let content_rect = rect![
            rect.min.x,
            rect.min.y,
            rect.max.x,
            rect.max.y - bar_height - separator_top_half
        ];

        let background = Filler::new(content_rect, WHITE);
        children.push(Box::new(background) as Box<dyn View>);

        let first_row_index = children.len();

        let row_height = scale_by_dpi(SMALL_BAR_HEIGHT, CURRENT_DEVICE.dpi) as i32;

        children.push(Self::build_bottom_separator(
            rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        children.push(Self::build_pagination_bar(
            rect,
            bar_height,
            separator_bottom_half,
            category,
            false,
            false,
        ));

        let keyboard = ToggleableKeyboard::new(rect, true);
        children.push(Box::new(keyboard) as Box<dyn View>);

        let separator_index = first_row_index;
        let keyboard_index = children.len() - 1;

        let dict_service = if category == Category::Dictionaries {
            match MonolingualDictionaryService::new(
                &context.database,
                std::path::Path::new(DICTIONARIES_DIRNAME),
            ) {
                Ok(service) => Some(service),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to create MonolingualDictionaryService");
                    None
                }
            }
        } else {
            None
        };

        let mut editor = CategoryEditor {
            id,
            rect,
            children,
            category,
            content_rect,
            row_height,
            focus: None,
            first_row_index,
            separator_index,
            keyboard_index,
            current_page: 0,
            pages_count: 1,
            dict_service,
        };

        editor.update_rows_list(rq, context);

        editor
    }

    pub fn category(&self) -> Category {
        self.category
    }

    #[inline]
    fn calculate_dimensions() -> (i32, i32, i32) {
        let dpi = CURRENT_DEVICE.dpi;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

        let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
        let bar_height = small_height;

        (bar_height, separator_top_half, separator_bottom_half)
    }

    #[inline]
    fn build_setting_row(
        kind: Box<dyn super::kinds::SettingKind>,
        row_rect: Rectangle,
        settings: &Settings,
        fonts: &mut crate::font::Fonts,
    ) -> Box<dyn View> {
        let setting_row = SettingRow::new(kind, row_rect, settings, fonts);
        Box::new(setting_row) as Box<dyn View>
    }

    #[inline]
    fn build_bottom_separator(
        rect: Rectangle,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                rect.min.x,
                rect.max.y - bar_height - separator_top_half,
                rect.max.x,
                rect.max.y - bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    #[inline]
    fn build_pagination_bar(
        rect: Rectangle,
        bar_height: i32,
        separator_bottom_half: i32,
        category: Category,
        prev_enabled: bool,
        next_enabled: bool,
    ) -> Box<dyn View> {
        let bottom_bar_rect = rect![
            rect.min.x,
            rect.max.y - bar_height + separator_bottom_half,
            rect.max.x,
            rect.max.y
        ];

        match category {
            Category::Libraries => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::PaginationWithButton {
                    prev_enabled,
                    next_enabled,
                    center_event: Event::AddLibrary,
                    center_icon: "plus",
                },
            )),
            _ => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::Pagination {
                    prev_enabled,
                    next_enabled,
                },
            )),
        }
    }

    /// Rebuilds the visible setting rows for the current page and refreshes the
    /// bottom navigation bar to reflect the new pagination state.
    ///
    /// The children vector always has the following fixed tail (3 items):
    /// `[..rows.., separator, bottom_bar, keyboard]`. This method drains the
    /// row slice, computes how many rows fit in `content_rect` at `row_height`,
    /// calculates `pages_count`, clamps `current_page` into bounds, inserts the
    /// rows for the current page, and replaces the separator and bottom bar with
    /// freshly-built ones whose prev/next arrows reflect whether adjacent pages
    /// exist. `keyboard_index` is re-synced at the end.
    ///
    /// This is the single mutation point for page state — both `rebuild_library_rows`
    /// and the `Event::Page` handler delegate here.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn update_rows_list(&mut self, rq: &mut RenderQueue, context: &mut Context) {
        self.children
            .drain(self.first_row_index..self.separator_index);

        let available_height = self.content_rect.height() as i32;
        let max_rows = (available_height / self.row_height).max(1) as usize;

        let all_kinds = self.category.settings(context, self.dict_service.as_ref());
        let total_rows = all_kinds.len();

        self.pages_count = total_rows.div_ceil(max_rows).max(1);
        self.current_page = self.current_page.min(self.pages_count - 1);

        let start = self.current_page * max_rows;
        let end = (start + max_rows).min(total_rows);

        let mut current_y = self.content_rect.min.y;
        let mut new_rows: Vec<Box<dyn View>> = Vec::new();

        for kind in all_kinds.into_iter().skip(start).take(end - start) {
            let row_rect = rect![
                self.content_rect.min.x,
                current_y,
                self.content_rect.max.x,
                current_y + self.row_height
            ];
            new_rows.push(Self::build_setting_row(
                kind,
                row_rect,
                &context.settings,
                &mut context.fonts,
            ));
            current_y += self.row_height;
        }

        let rows_len = new_rows.len();
        for (offset, row) in new_rows.into_iter().enumerate() {
            self.children.insert(self.first_row_index + offset, row);
        }

        self.separator_index = self.first_row_index + rows_len;
        let bar_index = self.separator_index + 1;
        self.children.remove(bar_index);
        self.children.remove(self.separator_index);

        let (bar_height, separator_top_half, separator_bottom_half) = Self::calculate_dimensions();
        let new_sep = Self::build_bottom_separator(
            self.rect,
            bar_height,
            separator_top_half,
            separator_bottom_half,
        );
        self.children.insert(self.separator_index, new_sep);

        let prev_enabled = self.current_page > 0;
        let next_enabled = self.current_page + 1 < self.pages_count;
        let new_bar = Self::build_pagination_bar(
            self.rect,
            bar_height,
            separator_bottom_half,
            self.category,
            prev_enabled,
            next_enabled,
        );
        self.children.insert(self.separator_index + 1, new_bar);

        self.keyboard_index = self.separator_index + 2;

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    /// Rebuilds the library rows in the UI after a library is added, removed, or modified.
    ///
    /// Resets to page 0 to avoid stale page state when the library list changes.
    #[inline]
    fn rebuild_library_rows(&mut self, rq: &mut RenderQueue, context: &mut Context) {
        if self.category != Category::Libraries {
            return;
        }

        self.current_page = 0;
        self.update_rows_list(rq, context);
    }

    #[inline]
    fn toggle_keyboard(
        &mut self,
        visible: bool,
        _id: Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        let keyboard = self.children[self.keyboard_index]
            .downcast_mut::<ToggleableKeyboard>()
            .expect("keyboard_index points to non-ToggleableKeyboard view");
        keyboard.set_visible(visible, hub, rq, context);
    }

    #[inline]
    fn handle_focus_event(
        &mut self,
        view_id: &Option<ViewId>,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.focus != *view_id {
            self.focus = *view_id;
            if view_id.is_some() {
                self.toggle_keyboard(true, *view_id, hub, rq, context);
            } else {
                self.toggle_keyboard(false, None, hub, rq, context);
            }
        }
        true
    }

    #[inline]
    fn handle_submenu_event(
        &mut self,
        rect: &Rectangle,
        entries: &[EntryKind],
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let menu = Menu::new(
            *rect,
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
    fn handle_delete_library(
        &mut self,
        index: usize,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if index < context.settings.libraries.len() {
            context.settings.libraries.remove(index);
            self.rebuild_library_rows(rq, context);
        }

        if let Some(menu_index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(menu_index);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }

        true
    }

    /// Handles the `AddLibrary` event by opening a `LibraryEditor` overlay for a new library.
    ///
    /// A `LibrarySettings` with default values is constructed and passed to the editor, but it
    /// is not written to `context.settings.libraries` here. The library is only committed when
    /// the user validates via `Event::UpdateLibrary`. The `LibraryEditor` is pushed to the end
    /// of the children array, after the keyboard, so `keyboard_index` remains valid.
    #[inline]
    fn handle_add_library_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let library = LibrarySettings {
            name: "untitled".to_string(),
            path: PathBuf::from("/mnt/onboard"),
            ..Default::default()
        };

        let library_editor = LibraryEditor::new(
            self.rect,
            context.settings.libraries.len(),
            library,
            hub,
            rq,
            context,
        );
        self.children.push(Box::new(library_editor));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        true
    }

    /// Handles the `EditLibrary` event by opening a `LibraryEditor` overlay for the specified library.
    ///
    /// This function creates a `LibraryEditor` view that allows the user to modify an existing
    /// library's settings (name, path, mode, etc.). The editor is pushed as a child view,
    /// creating an overlay on top of the category editor. The `LibraryEditor` is pushed to the
    /// end of the children array, after the keyboard, so `keyboard_index` remains valid.
    #[inline]
    fn handle_edit_library_event(
        &mut self,
        index: usize,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if let Some(library) = context.settings.libraries.get(index).cloned() {
            let library_editor = LibraryEditor::new(self.rect, index, library, hub, rq, context);
            self.children.push(Box::new(library_editor));
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
        true
    }

    #[inline]
    fn handle_update_library_event(
        &mut self,
        index: usize,
        library: &LibrarySettings,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if index < context.settings.libraries.len() {
            context.settings.libraries[index] = library.clone();
        } else if index == context.settings.libraries.len() {
            context.settings.libraries.push(library.clone());
        } else {
            return true;
        }

        self.rebuild_library_rows(rq, context);
        true
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn handle_open_named_input(
        &mut self,
        view_id: ViewId,
        label: String,
        max_chars: usize,
        initial_text: String,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let mut named_input =
            crate::view::named_input::NamedInput::new(label, view_id, view_id, max_chars, context);
        named_input.set_text(&initial_text, rq, context);
        self.children.push(Box::new(named_input));
        hub.send(Event::Focus(Some(view_id))).ok();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn handle_open_refresh_rate_editor_event(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let editor = RefreshRateByKindEditor::new(self.rect, hub, rq, context);
        self.children.push(Box::new(editor));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    /// Spawns a background thread to download and install a dictionary for the
    /// given language code.
    ///
    /// Uses `include_etymologies = false` to prefer the smaller no-etymology
    /// variant. Progress is reported via a sticky pinned notification that is
    /// dismissed when the download completes. On completion the thread sends
    /// `Event::DictionaryInstallComplete` via the hub so the UI can rebuild
    /// the rows on the main thread.
    ///
    /// If a download is already in progress for `lang` the request is silently
    /// ignored to prevent duplicate background threads racing to write the same
    /// files.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context)))]
    fn handle_download_dictionary(
        &mut self,
        lang: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let Some(service) = self.dict_service.clone() else {
            tracing::warn!(
                lang,
                "No MonolingualDictionaryService available to download dictionary"
            );
            self.update_rows_list(rq, context);
            return true;
        };

        let entry = match service.get_entry_for_lang(lang) {
            Ok(Some(e)) => e,
            Ok(None) => {
                tracing::warn!(lang, "No metadata entry found for language; cannot install");
                self.update_rows_list(rq, context);
                return true;
            }
            Err(e) => {
                tracing::warn!(lang, error = %e, "Failed to look up dictionary entry");
                self.update_rows_list(rq, context);
                return true;
            }
        };

        if !service.try_begin_install(lang) {
            self.update_rows_list(rq, context);
            return true;
        }

        self.update_rows_list(rq, context);

        let lang_owned = lang.to_string();
        let hub2 = hub.clone();
        let parent_span = tracing::Span::current();

        let download_id = ViewId::MessageNotif(ID_FEEDER.next());
        hub.send(Event::Notification(NotificationEvent::ShowPinned(
            download_id,
            fl!("notification-downloading-dictionary", lang = lang),
        )))
        .ok();

        thread::spawn(move || {
            let _span =
                tracing::info_span!(parent: &parent_span, "dictionary_install_async").entered();

            let result = service
                .install_reserved_dictionary(
                    &lang_owned,
                    &entry,
                    false,
                    &mut |downloaded, total| {
                        use humanize_bytes::humanize_bytes_decimal;
                        let downloaded_str = humanize_bytes_decimal!(downloaded).to_string();
                        let total_str = humanize_bytes_decimal!(total).to_string();
                        let percent = (downloaded as f64 / total as f64 * 100.0) as u8;
                        hub2.send(Event::Notification(NotificationEvent::UpdateProgress(
                            download_id,
                            percent,
                        )))
                        .ok();
                        hub2.send(Event::Notification(NotificationEvent::UpdateText(
                            download_id,
                            fl!(
                                "notification-downloading-dictionary-progress",
                                lang = lang_owned.as_str(),
                                downloaded = downloaded_str.as_str(),
                                total = total_str.as_str()
                            ),
                        )))
                        .ok();
                    },
                )
                .map_err(|e| e.to_string());

            hub2.send(Event::Close(download_id)).ok();
            hub2.send(crate::view::Event::DictionaryInstallComplete {
                lang: lang_owned,
                result,
            })
            .ok();
        });

        true
    }

    /// Opens a modal confirmation dialog before starting a dictionary download.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context)))]
    fn handle_dictionary_download_request(
        &mut self,
        lang: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if !context.online {
            hub.send(Event::Notification(NotificationEvent::Show(fl!(
                "notification-not-online"
            ))))
            .ok();

            return true;
        }

        self.remove_dictionary_download_confirm(rq);

        let dialog = Dialog::builder(
            ViewId::DictionaryDownloadConfirm,
            fl!("settings-dictionaries-confirm-download", lang = lang),
        )
        .add_button(
            &fl!("settings-dictionaries-confirm-download-cancel"),
            Event::Close(ViewId::DictionaryDownloadConfirm),
        )
        .add_button(
            &fl!("settings-dictionaries-confirm-download-confirm"),
            Event::Select(EntryId::DownloadDictionary(lang.to_string())),
        )
        .build(context);

        rq.add(RenderData::new(
            dialog.id(),
            *dialog.rect(),
            UpdateMode::Gui,
        ));
        self.children.push(Box::new(dialog));
        true
    }

    /// Removes any active dictionary download confirmation dialog.
    #[inline]
    fn remove_dictionary_download_confirm(&mut self, rq: &mut RenderQueue) {
        if let Some(index) = locate_by_id(self, ViewId::DictionaryDownloadConfirm) {
            let dialog_rect = *self.children[index].rect();
            self.children.remove(index);
            rq.add(RenderData::expose(dialog_rect, UpdateMode::Gui));
        }
    }

    /// Starts a confirmed dictionary download and closes the confirmation dialog.
    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, rq, context)))]
    fn handle_confirm_dictionary_download(
        &mut self,
        lang: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        self.remove_dictionary_download_confirm(rq);

        if !context.online {
            hub.send(Event::Notification(NotificationEvent::Show(fl!(
                "notification-not-online"
            ))))
            .ok();

            return true;
        }

        self.handle_download_dictionary(lang, hub, rq, context)
    }

    /// Removes the installed dictionary directory for the given language code, then rebuilds the rows.
    ///
    /// The directory `<DICTIONARIES_DIRNAME>/reader-dict/<lang>/` is removed. Any open
    /// `SettingsValueMenu` is closed before rebuilding. Logs a warning on failure.
    #[inline]
    fn handle_delete_dictionary(
        &mut self,
        lang: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        let lang_dir = Path::new(DICTIONARIES_DIRNAME)
            .join("reader-dict")
            .join(lang);

        if lang_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&lang_dir) {
                tracing::warn!(lang, error = %e, "Failed to delete dictionary directory");
            }
        }

        if let Some(service) = &self.dict_service {
            service.remove_installed(lang);
        }

        if let Some(menu_index) = locate_by_id(self, ViewId::SettingsValueMenu) {
            self.children.remove(menu_index);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }

        self.current_page = 0;
        self.update_rows_list(rq, context);
        hub.send(Event::ReindexDictionaries).ok();
        true
    }

    #[inline]
    fn handle_close_view_event(
        &mut self,
        view_id: &ViewId,
        hub: &Hub,
        rq: &mut RenderQueue,
    ) -> bool {
        match view_id {
            ViewId::AutoSuspendInput
            | ViewId::AutoPowerOffInput
            | ViewId::SettingsRetentionInput
            | ViewId::SettingsValueMenu
            | ViewId::LibraryEditor
            | ViewId::DictionaryDownloadConfirm
            | ViewId::RefreshRateByKindEditor => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            #[cfg(feature = "tracing")]
            ViewId::OtlpEndpointInput => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    let input_rect = *self.children[index].rect();
                    self.children.remove(index);
                    rq.add(RenderData::expose(input_rect, UpdateMode::Gui));
                }
                hub.send(Event::Focus(None)).ok();
                true
            }
            #[cfg(feature = "profiling")]
            ViewId::PyroscopeEndpointInput => {
                if let Some(index) = locate_by_id(self, *view_id) {
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

impl View for CategoryEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Focus(view_id) => self.handle_focus_event(view_id, hub, rq, context),
            Event::SubMenu(rect, entries) => self.handle_submenu_event(rect, entries, rq, context),
            Event::Select(EntryId::DeleteLibrary(index)) => {
                self.handle_delete_library(*index, rq, context)
            }
            Event::Select(EntryId::RequestDictionaryDownload(lang)) => {
                self.handle_dictionary_download_request(lang, hub, rq, context)
            }
            Event::Select(EntryId::DownloadDictionary(lang)) => {
                self.handle_confirm_dictionary_download(lang, hub, rq, context)
            }
            Event::Select(EntryId::DeleteDictionary(lang)) => {
                self.handle_delete_dictionary(lang, hub, rq, context)
            }
            Event::AddLibrary => self.handle_add_library_event(hub, rq, context),
            Event::EditLibrary(index) => self.handle_edit_library_event(*index, hub, rq, context),
            Event::OpenRefreshRateEditor => {
                self.handle_open_refresh_rate_editor_event(hub, rq, context)
            }
            Event::UpdateLibrary(index, library) => {
                self.handle_update_library_event(*index, library, rq, context)
            }
            Event::OpenNamedInput {
                view_id,
                label,
                max_chars,
                initial_text,
            } => self.handle_open_named_input(
                *view_id,
                label.clone(),
                *max_chars,
                initial_text.clone(),
                hub,
                rq,
                context,
            ),
            Event::Close(view_id) => self.handle_close_view_event(view_id, hub, rq),
            Event::DictionaryInstallComplete { lang, result } => {
                match result {
                    Ok(()) => {
                        tracing::info!(lang, "Dictionary installed");

                        self.current_page = 0;
                        self.update_rows_list(rq, context);
                        hub.send(Event::ReindexDictionaries).ok();

                        hub.send(Event::Notification(NotificationEvent::Show(fl!(
                            "notification-downloading-dictionary-completed",
                            lang = lang
                        ))))
                        .ok();
                    }
                    Err(e) => {
                        tracing::warn!(lang, error = %e, "Failed to install dictionary");

                        self.current_page = 0;
                        self.update_rows_list(rq, context);

                        hub.send(Event::Notification(NotificationEvent::Show(fl!(
                            "notification-dictionary-install-failed",
                            lang = lang
                        ))))
                        .ok();
                    }
                }

                true
            }
            Event::Page(dir) => {
                let new_page = match dir {
                    CycleDir::Previous => self.current_page.saturating_sub(1),
                    CycleDir::Next => {
                        (self.current_page + 1).min(self.pages_count.saturating_sub(1))
                    }
                };
                if new_page != self.current_page {
                    self.current_page = new_page;
                    self.update_rows_list(rq, context);
                }
                true
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

    fn is_background(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::crypto;
    use crate::geom::Point;
    use crate::gesture::GestureEvent;
    use crate::settings::Settings;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_settings_with_libraries(count: usize) -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        for i in 0..count {
            settings.libraries.push(LibrarySettings {
                name: format!("Library {}", i),
                path: PathBuf::from(format!("/mnt/onboard/lib{}", i)),
                ..Default::default()
            });
        }
        settings
    }

    fn create_test_category_editor_with_context(context: &mut Context) -> CategoryEditor {
        let rect = rect![0, 0, 600, 800];
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Libraries, &mut rq, context)
    }

    fn create_test_dictionary_category_editor(context: &mut Context) -> CategoryEditor {
        crypto::init_crypto_provider();
        let rect = rect![0, 0, 600, 800];
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Dictionaries, &mut rq, context)
    }

    #[test]
    fn test_add_library_event() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 0);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 0);

        assert_eq!(
            editor.children.len(),
            initial_children_count + 1,
            "Expected +1: one library editor"
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_add_library_preserves_structural_children() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 2);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 2);

        assert_eq!(
            editor.children.len(),
            initial_children_count + 1,
            "Expected children count to increase by 1: one library editor"
        );

        assert_eq!(
            // minus 2 to account for the newly added library editor
            editor.keyboard_index,
            editor.children.len() - 2,
            "keyboard_index should point to the last child (the keyboard)"
        );

        assert!(
            editor.keyboard_index < editor.children.len(),
            "keyboard_index out of bounds - structural children were likely removed incorrectly"
        );

        let keyboard_still_exists = editor
            .children
            .iter()
            .any(|child| child.downcast_ref::<ToggleableKeyboard>().is_some());

        assert!(
            keyboard_still_exists,
            "ToggleableKeyboard view should still exist in children after adding library"
        );
    }

    #[test]
    fn test_delete_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 2);
        assert_eq!(context.settings.libraries[0].name, "Library 0");
        assert_eq!(context.settings.libraries[1].name, "Library 1");

        let row_y = editor.content_rect.min.y + (editor.row_height / 2);
        let point = Point::new(editor.content_rect.min.x + 10, row_y);

        editor.handle_event(
            &Event::Gesture(GestureEvent::HoldFingerShort(point, 0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        rq = RenderQueue::new();

        let handled = editor.handle_event(
            &Event::Select(EntryId::DeleteLibrary(0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Library 1");

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Library 0");

        let updated_library = LibrarySettings {
            name: "Updated Library".to_string(),
            path: PathBuf::from("/mnt/onboard/updated"),
            ..Default::default()
        };

        let handled = editor.handle_event(
            &Event::UpdateLibrary(0, Box::new(updated_library.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Updated Library");
        assert_eq!(
            context.settings.libraries[0].path,
            PathBuf::from("/mnt/onboard/updated")
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_edit_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let handled = editor.handle_event(
            &Event::EditLibrary(0),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(!rq.is_empty());
    }

    fn create_test_intermissions_category_editor(context: &mut Context) -> CategoryEditor {
        let rect = rect![0, 0, 600, 800];
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Intermissions, &mut rq, context)
    }

    #[test]
    fn test_set_intermission_logo() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = crate::view::handle_event(
            &mut editor,
            &Event::Select(EntryId::SetIntermission(
                IntermKind::Suspend,
                IntermissionDisplay::Logo,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(matches!(
            context.settings.intermissions[IntermKind::Suspend],
            IntermissionDisplay::Logo
        ));
    }

    #[test]
    fn test_set_intermission_cover() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = crate::view::handle_event(
            &mut editor,
            &Event::Select(EntryId::SetIntermission(
                IntermKind::PowerOff,
                IntermissionDisplay::Cover,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(matches!(
            context.settings.intermissions[IntermKind::PowerOff],
            IntermissionDisplay::Cover
        ));
    }

    #[test]
    fn test_set_intermission_blank_inverted() {
        use crate::settings::{IntermKind, IntermissionDisplay};

        let mut context = create_test_context();
        let mut editor = create_test_intermissions_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = crate::view::handle_event(
            &mut editor,
            &Event::Select(EntryId::SetIntermission(
                IntermKind::Share,
                IntermissionDisplay::BlankInverted,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(matches!(
            context.settings.intermissions[IntermKind::Share],
            IntermissionDisplay::BlankInverted
        ));
    }

    #[test]
    fn test_pagination_children_structure() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        let editor = create_test_category_editor_with_context(&mut context);

        let keyboard_still_exists = editor
            .children
            .iter()
            .any(|child| child.downcast_ref::<ToggleableKeyboard>().is_some());
        assert!(
            keyboard_still_exists,
            "ToggleableKeyboard must always be present"
        );
        assert_eq!(
            editor.keyboard_index,
            editor.children.len() - 1,
            "keyboard_index must point to the last child"
        );
    }

    #[test]
    fn test_page_navigation_event() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        for i in 0..50 {
            context.settings.libraries.push(LibrarySettings {
                name: format!("Library {}", i),
                path: PathBuf::from(format!("/mnt/onboard/lib{}", i)),
                ..Default::default()
            });
        }

        let mut editor = create_test_category_editor_with_context(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.current_page, 0);
        assert!(
            editor.pages_count > 1,
            "Should have multiple pages with 50 libraries"
        );

        let handled = editor.handle_event(
            &Event::Page(CycleDir::Next),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        assert!(handled);
        assert_eq!(editor.current_page, 1);

        let handled = editor.handle_event(
            &Event::Page(CycleDir::Previous),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        assert!(handled);
        assert_eq!(editor.current_page, 0);

        let handled = editor.handle_event(
            &Event::Page(CycleDir::Previous),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        assert!(handled);
        assert_eq!(editor.current_page, 0, "Should not go below page 0");
    }

    #[test]
    fn test_download_dictionary_opens_confirmation_dialog() {
        let mut context = create_test_context();
        context.online = true;
        let mut editor = create_test_dictionary_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let handled = editor.handle_event(
            &Event::Select(EntryId::RequestDictionaryDownload("xy".to_string())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled, "Download event should be handled");
        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(
            locate_by_id(&editor, ViewId::DictionaryDownloadConfirm).is_some(),
            "Dictionary download confirmation dialog should be present"
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_redownload_dictionary_opens_confirmation_dialog() {
        let mut context = create_test_context();
        context.online = true;
        let mut editor = create_test_dictionary_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let handled = editor.handle_event(
            &Event::Select(EntryId::RequestDictionaryDownload("xy".to_string())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled, "Re-download event should be handled");
        assert!(
            locate_by_id(&editor, ViewId::DictionaryDownloadConfirm).is_some(),
            "Dictionary download confirmation dialog should be present"
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_close_dictionary_download_confirmation_removes_dialog() {
        let mut context = create_test_context();
        context.online = true;
        let mut editor = create_test_dictionary_category_editor(&mut context);
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        editor.handle_event(
            &Event::Select(EntryId::RequestDictionaryDownload("xy".to_string())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        let handled = editor.handle_event(
            &Event::Close(ViewId::DictionaryDownloadConfirm),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled, "Close event should be handled");
        assert!(
            locate_by_id(&editor, ViewId::DictionaryDownloadConfirm).is_none(),
            "Dictionary download confirmation dialog should be removed"
        );
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_download_dictionary_invalid_language_rebuilds_rows() {
        let mut context = create_test_context();
        context.online = true;
        let mut editor = create_test_dictionary_category_editor(&mut context);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq2 = RenderQueue::new();

        let initial_rows_count = editor
            .separator_index
            .saturating_sub(editor.first_row_index);

        editor.handle_event(
            &Event::Select(EntryId::RequestDictionaryDownload("xy".to_string())),
            &hub,
            &mut bus,
            &mut rq2,
            &mut context,
        );

        let handled = editor.handle_event(
            &Event::Select(EntryId::DownloadDictionary("xy".to_string())),
            &hub,
            &mut bus,
            &mut rq2,
            &mut context,
        );

        assert!(handled, "Download event should be handled");
        assert!(
            locate_by_id(&editor, ViewId::DictionaryDownloadConfirm).is_none(),
            "Dictionary download confirmation dialog should be removed"
        );

        let rows_count_after = editor
            .separator_index
            .saturating_sub(editor.first_row_index);

        assert_eq!(
            initial_rows_count, rows_count_after,
            "Row count unchanged after download attempt (early return triggers rebuild)"
        );
        assert!(
            !rq2.is_empty(),
            "RenderQueue should have render requests from update_rows_list()"
        );
    }
}
