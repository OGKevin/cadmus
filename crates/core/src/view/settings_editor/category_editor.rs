use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, Rectangle};
use crate::gesture::GestureEvent;
use crate::settings::{LibraryMode, LibrarySettings, Settings};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::filler::Filler;
use crate::view::menu::Menu;
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, BIG_BAR_HEIGHT,
    ID_FEEDER, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use anyhow::Error;
use std::path::PathBuf;

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};
use super::category::Category;
use super::library_editor::LibraryEditor;
use super::setting_row::{Kind as RowKind, SettingRow};

pub struct CategoryEditor {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    #[allow(dead_code)]
    category: Category,
    settings: Settings,
    original_settings: Settings,
    content_rect: Rectangle,
    row_height: i32,
}

pub struct CategoryEditorBuilder<'a> {
    rect: Rectangle,
    category: Category,
    rq: &'a mut RenderQueue,
    context: &'a mut Context,
    settings: Settings,
}

impl<'a> CategoryEditorBuilder<'a> {
    fn new(
        rect: Rectangle,
        category: Category,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> Self {
        CategoryEditorBuilder {
            rect,
            category,
            rq,
            settings: context.settings.clone(),
            context,
        }
    }

    fn calculate_dimensions(&self) -> (i32, i32, i32, i32) {
        let dpi = CURRENT_DEVICE.dpi;
        let (small_height, _big_height) = (
            scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32,
            scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32,
        );
        let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
        let bar_height = small_height;

        (
            bar_height,
            separator_thickness,
            separator_top_half,
            separator_bottom_half,
        )
    }

    fn build_top_bar(&mut self, bar_height: i32, separator_top_half: i32) -> Box<dyn View> {
        let top_bar = TopBar::new(
            rect![
                self.rect.min.x,
                self.rect.min.y,
                self.rect.max.x,
                self.rect.min.y + bar_height - separator_top_half
            ],
            TopBarVariant::Cancel(Event::Close(ViewId::SettingsCategoryEditor)),
            self.category.label(),
            self.context,
        );
        Box::new(top_bar) as Box<dyn View>
    }

    fn build_top_separator(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                self.rect.min.x,
                self.rect.min.y + bar_height - separator_top_half,
                self.rect.max.x,
                self.rect.min.y + bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    fn build_content_background(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> (Box<dyn View>, Rectangle) {
        let content_rect = rect![
            self.rect.min.x,
            self.rect.min.y + bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y - bar_height - separator_top_half
        ];

        let background = Filler::new(content_rect, WHITE);
        (Box::new(background) as Box<dyn View>, content_rect)
    }

    fn build_setting_row(
        &mut self,
        kind: RowKind,
        row_rect: Rectangle,
    ) -> Result<Box<dyn View>, Error> {
        let setting_row = SettingRow::new(kind, row_rect, &self.settings);
        Ok(Box::new(setting_row) as Box<dyn View>)
    }

    fn build_bottom_separator(
        &mut self,
        bar_height: i32,
        separator_top_half: i32,
        separator_bottom_half: i32,
    ) -> Box<dyn View> {
        let separator = Filler::new(
            rect![
                self.rect.min.x,
                self.rect.max.y - bar_height - separator_top_half,
                self.rect.max.x,
                self.rect.max.y - bar_height + separator_bottom_half
            ],
            BLACK,
        );
        Box::new(separator) as Box<dyn View>
    }

    fn build_bottom_bar(&mut self, bar_height: i32, separator_bottom_half: i32) -> Box<dyn View> {
        let bottom_bar_rect = rect![
            self.rect.min.x,
            self.rect.max.y - bar_height + separator_bottom_half,
            self.rect.max.x,
            self.rect.max.y
        ];

        match self.category {
            Category::Libraries => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::TwoButtons {
                    left_event: Event::AddLibrary,
                    left_icon: "plus",
                    right_event: Event::Validate,
                    right_icon: "check_mark-large",
                },
            )),
            _ => Box::new(SettingsEditorBottomBar::new(
                bottom_bar_rect,
                BottomBarVariant::SingleButton {
                    event: Event::Validate,
                    icon: "check_mark-large",
                },
            )),
        }
    }

    pub fn build(mut self) -> Result<CategoryEditor, Error> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let (bar_height, _separator_thickness, separator_top_half, separator_bottom_half) =
            self.calculate_dimensions();

        children.push(self.build_top_bar(bar_height, separator_top_half));

        children.push(self.build_top_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        let (background, content_rect) =
            self.build_content_background(bar_height, separator_top_half, separator_bottom_half);
        children.push(background);

        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, CURRENT_DEVICE.dpi) as i32;
        let setting_kinds = self.category.settings(self.context);
        let mut current_y = content_rect.min.y;

        for kind in setting_kinds {
            let row_rect = rect![
                content_rect.min.x,
                current_y,
                content_rect.max.x,
                current_y + row_height
            ];
            children.push(self.build_setting_row(kind, row_rect)?);
            current_y += row_height;
        }

        children.push(self.build_bottom_separator(
            bar_height,
            separator_top_half,
            separator_bottom_half,
        ));

        children.push(self.build_bottom_bar(bar_height, separator_bottom_half));

        self.rq.add(RenderData::new(id, self.rect, UpdateMode::Gui));

        Ok(CategoryEditor {
            id,
            rect: self.rect,
            children,
            category: self.category,
            original_settings: self.settings.clone(),
            settings: self.settings,
            content_rect,
            row_height,
        })
    }
}

impl CategoryEditor {
    pub fn new<'a>(
        rect: Rectangle,
        category: Category,
        _hub: &'a Hub,
        rq: &'a mut RenderQueue,
        context: &'a mut Context,
    ) -> CategoryEditorBuilder<'a> {
        CategoryEditorBuilder::new(rect, category, rq, context)
    }

    fn rebuild_library_rows(
        &mut self,
        rq: &mut RenderQueue,
        original_count: Option<usize>,
    ) -> Result<(), Error> {
        if self.category != Category::Libraries {
            return Ok(());
        }

        let num_libraries = self.settings.libraries.len();
        let rows_to_remove = original_count.unwrap_or(num_libraries);

        let first_row_index = 3;

        for _ in 0..rows_to_remove {
            if first_row_index < self.children.len() {
                self.children.remove(first_row_index);
            }
        }

        let mut current_y = self.content_rect.min.y;
        let mut new_rows = Vec::new();

        for i in 0..num_libraries {
            let row_rect = rect![
                self.content_rect.min.x,
                current_y,
                self.content_rect.max.x,
                current_y + self.row_height
            ];

            let setting_row = SettingRow::new(RowKind::Library(i), row_rect, &self.settings);

            new_rows.push(Box::new(setting_row) as Box<dyn View>);
            current_y += self.row_height;
        }

        for (offset, row) in new_rows.into_iter().enumerate() {
            self.children.insert(first_row_index + offset, row);
        }

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

        Ok(())
    }
}

impl View for CategoryEditor {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::Back => {
                bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
                true
            }
            Event::Gesture(GestureEvent::HoldFingerShort(point, _)) => {
                if self.category != Category::Libraries {
                    return false;
                }

                if !self.content_rect.includes(*point) {
                    return false;
                }

                let row_index = (point.y - self.content_rect.min.y) / self.row_height;
                let library_index = row_index as usize;

                if library_index < self.settings.libraries.len() {
                    let row_y = self.content_rect.min.y + (row_index * self.row_height);
                    let row_rect = rect![
                        self.content_rect.min.x,
                        row_y,
                        self.content_rect.max.x,
                        row_y + self.row_height
                    ];

                    let entries = vec![EntryKind::Command(
                        "Delete".to_string(),
                        EntryId::DeleteLibrary(library_index),
                    )];

                    bus.push_back(Event::SubMenu(row_rect, entries));
                    return true;
                }

                false
            }
            Event::SubMenu(rect, ref entries) => {
                let menu = Menu::new(
                    *rect,
                    ViewId::SettingsValueMenu,
                    crate::view::menu::MenuKind::SubMenu,
                    entries.clone(),
                    context,
                );

                rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
                self.children.push(Box::new(menu));

                true
            }
            Event::Select(ref id) => match id {
                EntryId::SetKeyboardLayout(ref layout) => {
                    self.settings.keyboard_layout = layout.clone();
                    false
                }
                EntryId::ToggleSleepCover => {
                    self.settings.sleep_cover = !self.settings.sleep_cover;
                    false
                }
                EntryId::ToggleAutoShare => {
                    self.settings.auto_share = !self.settings.auto_share;
                    false
                }
                EntryId::SetButtonScheme(button_scheme) => {
                    self.settings.button_scheme = *button_scheme;
                    false
                }
                EntryId::DeleteLibrary(index) => {
                    if *index < self.settings.libraries.len() {
                        let original_count = self.settings.libraries.len();
                        self.settings.libraries.remove(*index);

                        if let Err(e) = self.rebuild_library_rows(rq, Some(original_count)) {
                            eprintln!("Failed to rebuild library rows: {}", e);
                        }
                    }

                    if let Some(menu_index) = locate_by_id(self, ViewId::SettingsValueMenu) {
                        self.children.remove(menu_index);
                        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    }

                    false
                }
                _ => false,
            },
            Event::Validate => {
                bus.push_back(Event::UpdateSettings(self.settings.clone()));
                bus.push_back(Event::Close(ViewId::SettingsCategoryEditor));
                true
            }
            Event::AddLibrary => {
                let library = LibrarySettings {
                    name: String::new(),
                    path: PathBuf::from("/mnt/onboard"),
                    mode: LibraryMode::Filesystem,
                    ..Default::default()
                };

                let library_index = self.settings.libraries.len();
                self.settings.libraries.push(library.clone());

                if let Err(e) = self.rebuild_library_rows(rq, None) {
                    eprintln!("Failed to rebuild library rows: {}", e);
                }

                if let Ok(library_editor) =
                    LibraryEditor::new(self.rect, library_index, library, hub, rq, context)
                {
                    self.children.push(Box::new(library_editor));
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }

                true
            }
            Event::EditLibrary(index) => {
                if let Some(library) = self.settings.libraries.get(*index).cloned() {
                    if let Ok(library_editor) =
                        LibraryEditor::new(self.rect, *index, library, hub, rq, context)
                    {
                        self.children.push(Box::new(library_editor));
                        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    }
                }
                true
            }
            Event::UpdateLibrary(index, ref library) => {
                if *index < self.settings.libraries.len() {
                    self.settings.libraries[*index] = (**library).clone();

                    if let Err(e) = self.rebuild_library_rows(rq, None) {
                        eprintln!("Failed to rebuild library rows: {}", e);
                    }
                }

                false
            }
            Event::Close(ViewId::LibraryEditor) => {
                if let Some(index) = locate_by_id(self, ViewId::LibraryEditor) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Close(view_id) if *view_id == ViewId::SettingsCategoryEditor => {
                self.settings = self.original_settings.clone();
                false
            }
            Event::Close(view_id) if *view_id == ViewId::SettingsValueMenu => {
                if let Some(index) = locate_by_id(self, *view_id) {
                    self.children.remove(index);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }

                true
            }
            _ => false,
        }
    }

    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut crate::font::Fonts) {
    }

    fn is_background(&self) -> bool {
        true
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::SettingsCategoryEditor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery::{Battery, FakeBattery};
    use crate::font::Fonts;
    use crate::framebuffer::Pixmap;
    use crate::frontlight::{Frontlight, LightLevels};
    use crate::geom::Point;
    use crate::library::Library;
    use crate::lightsensor::LightSensor;
    use crate::settings::{LibraryMode, Settings};
    use std::collections::VecDeque;
    use std::env;
    use std::path::Path;
    use std::sync::mpsc::channel;

    fn create_test_context() -> Context {
        let fb = Box::new(Pixmap::new(600, 800, 1)) as Box<dyn Framebuffer>;
        let battery = Box::new(FakeBattery::new()) as Box<dyn Battery>;
        let frontlight = Box::new(LightLevels::default()) as Box<dyn Frontlight>;
        let lightsensor = Box::new(0u16) as Box<dyn LightSensor>;
        let settings = Settings::default();
        let library = Library::new(Path::new("."), LibraryMode::Database).unwrap_or_else(|_| {
            Library::new(Path::new("/tmp"), LibraryMode::Database).expect(
                "Failed to create test library. \
                 Ensure /tmp directory exists and is writable.",
            )
        });
        let fonts = Fonts::load_from(
            Path::new(
                &env::var("TEST_ROOT_DIR").expect("TEST_ROOT_DIR must be set for this test."),
            )
            .to_path_buf(),
        )
        .expect(
            "Failed to load fonts. Tests require font files to be present. \
             Run tests from the project root directory.",
        );

        let mut ctx = Context::new(
            fb,
            None,
            library,
            settings,
            fonts,
            battery,
            frontlight,
            lightsensor,
        );
        ctx.load_keyboard_layouts();
        ctx.load_dictionaries();

        ctx
    }

    fn create_test_settings_with_libraries(count: usize) -> Settings {
        let mut settings = Settings::default();
        settings.libraries.clear();
        for i in 0..count {
            settings.libraries.push(LibrarySettings {
                name: format!("Library {}", i),
                path: PathBuf::from(format!("/mnt/onboard/lib{}", i)),
                mode: LibraryMode::Filesystem,
                ..Default::default()
            });
        }
        settings
    }

    fn create_test_category_editor_with_context(
        context: &mut Context,
    ) -> Result<CategoryEditor, Error> {
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();

        CategoryEditor::new(rect, Category::Libraries, &hub, &mut rq, context).build()
    }

    #[test]
    fn test_add_library_event() {
        let mut context = create_test_context();
        context.settings = Settings::default();
        context.settings.libraries.clear();
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 0);
        let initial_children_count = editor.children.len();

        let handled =
            editor.handle_event(&Event::AddLibrary, &hub, &mut bus, &mut rq, &mut context);

        assert!(handled);
        assert_eq!(editor.settings.libraries.len(), 1);

        let added_library = &editor.settings.libraries[0];
        assert_eq!(added_library.name, String::new());
        assert_eq!(added_library.path, PathBuf::from("/mnt/onboard"));
        assert_eq!(added_library.mode, LibraryMode::Filesystem);

        assert_eq!(editor.children.len(), initial_children_count + 1);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_delete_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(2);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 2);
        assert_eq!(editor.settings.libraries[0].name, "Library 0");
        assert_eq!(editor.settings.libraries[1].name, "Library 1");

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
        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Library 1");

        assert!(!rq.is_empty());
    }

    #[test]
    fn test_update_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Library 0");

        let updated_library = LibrarySettings {
            name: "Updated Library".to_string(),
            path: PathBuf::from("/mnt/onboard/updated"),
            mode: LibraryMode::Database,
            ..Default::default()
        };

        let handled = editor.handle_event(
            &Event::UpdateLibrary(0, Box::new(updated_library.clone())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert_eq!(editor.settings.libraries.len(), 1);
        assert_eq!(editor.settings.libraries[0].name, "Updated Library");
        assert_eq!(
            editor.settings.libraries[0].path,
            PathBuf::from("/mnt/onboard/updated")
        );
        assert_eq!(editor.settings.libraries[0].mode, LibraryMode::Database);
        assert!(!rq.is_empty());
    }

    #[test]
    fn test_edit_library_event() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
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

    #[test]
    fn test_hold_finger_shows_delete_menu() {
        let mut context = create_test_context();
        context.settings = create_test_settings_with_libraries(1);
        let mut editor = create_test_category_editor_with_context(&mut context)
            .expect("Failed to create category editor");
        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let initial_children_count = editor.children.len();

        let row_y = editor.content_rect.min.y + (editor.row_height / 2);
        let point = Point::new(editor.content_rect.min.x + 10, row_y);

        let handled = editor.handle_event(
            &Event::Gesture(GestureEvent::HoldFingerShort(point, 0)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert_eq!(bus.len(), 1);

        if let Some(Event::SubMenu(rect, entries)) = bus.pop_front() {
            assert_eq!(entries.len(), 1);
            match &entries[0] {
                EntryKind::Command(label, entry_id) => {
                    assert_eq!(label, "Delete");
                    assert_eq!(*entry_id, EntryId::DeleteLibrary(0));
                }
                _ => panic!("Expected Command entry"),
            }

            editor.handle_event(
                &Event::SubMenu(rect, entries),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            );

            assert_eq!(editor.children.len(), initial_children_count + 1);
            assert!(!rq.is_empty());
        } else {
            panic!("Expected SubMenu event in bus");
        }
    }
}
