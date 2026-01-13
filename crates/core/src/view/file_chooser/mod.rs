mod breadcrumb;
mod file_entry;

use self::breadcrumb::Breadcrumb;
pub use self::file_entry::FileEntry;

use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, CycleDir, Rectangle};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::label::Label;
use crate::view::page_label::PageLabel;
use crate::view::top_bar::TopBar;
use crate::view::{Bus, EntryId, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use crate::view::{BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct FileEntryData {
    pub path: PathBuf,
    pub name: String,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub is_dir: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SelectionMode {
    File,
    Directory,
    Both,
}

struct FileChooserLayout {
    thickness: i32,
    small_thickness: i32,
    big_thickness: i32,
    small_height: i32,
    big_height: i32,
}

impl FileChooserLayout {
    fn new(dpi: u16) -> Self {
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (small_thickness, big_thickness) = halves(thickness);
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let big_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;

        Self {
            thickness,
            small_thickness,
            big_thickness,
            small_height,
            big_height,
        }
    }

    fn top_bar_rect(&self, rect: &Rectangle) -> Rectangle {
        rect![
            rect.min.x,
            rect.min.y,
            rect.max.x,
            rect.min.y + self.small_height - self.small_thickness
        ]
    }

    fn first_separator_rect(&self, rect: &Rectangle) -> Rectangle {
        rect![
            rect.min.x,
            rect.min.y + self.small_height - self.small_thickness,
            rect.max.x,
            rect.min.y + self.small_height + self.big_thickness
        ]
    }

    fn breadcrumb_rect(&self, rect: &Rectangle) -> Rectangle {
        rect![
            rect.min.x,
            rect.min.y + self.small_height + self.big_thickness,
            rect.max.x,
            rect.min.y + self.small_height + self.big_thickness + self.small_height
                - self.thickness
        ]
    }

    fn second_separator_rect(&self, rect: &Rectangle) -> Rectangle {
        rect![
            rect.min.x,
            rect.min.y + 2 * self.small_height + self.big_thickness - self.thickness,
            rect.max.x,
            rect.min.y + 2 * self.small_height + self.big_thickness
        ]
    }
}

pub struct FileChooser {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    current_path: PathBuf,
    entries: Vec<FileEntryData>,
    current_page: usize,
    pages_count: usize,
    mode: SelectionMode,
    breadcrumb_index: usize,
    entries_start_index: usize,
    error_message: Option<String>,
}

impl FileChooser {
    fn create_separator(rect: Rectangle) -> Box<dyn View> {
        Box::new(Filler::new(rect, BLACK))
    }

    fn get_title_for_mode(mode: SelectionMode) -> &'static str {
        match mode {
            SelectionMode::File => "Select File",
            SelectionMode::Directory => "Select Folder",
            SelectionMode::Both => "Select File or Folder",
        }
    }

    fn build_children(
        rect: Rectangle,
        initial_path: &PathBuf,
        mode: SelectionMode,
        layout: &FileChooserLayout,
        context: &mut Context,
    ) -> (Vec<Box<dyn View>>, usize) {
        let mut children = Vec::new();

        let background = Filler::new(rect, WHITE);
        children.push(Box::new(background) as Box<dyn View>);

        let title = Self::get_title_for_mode(mode);
        let top_bar = TopBar::new(
            layout.top_bar_rect(&rect),
            Event::Close(ViewId::FileChooser),
            title.to_string(),
            context,
        );
        children.push(Box::new(top_bar) as Box<dyn View>);

        children.push(Self::create_separator(layout.first_separator_rect(&rect)));

        let breadcrumb_index = children.len();
        let breadcrumb = Breadcrumb::new(layout.breadcrumb_rect(&rect), initial_path);
        children.push(Box::new(breadcrumb) as Box<dyn View>);

        children.push(Self::create_separator(layout.second_separator_rect(&rect)));

        (children, breadcrumb_index)
    }

    pub fn new(
        rect: Rectangle,
        initial_path: PathBuf,
        mode: SelectionMode,
        _hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> FileChooser {
        let id = ID_FEEDER.next();
        let dpi = CURRENT_DEVICE.dpi;
        let layout = FileChooserLayout::new(dpi);

        let (children, breadcrumb_index) =
            Self::build_children(rect, &initial_path, mode, &layout, context);
        let entries_start_index = children.len();

        rq.add(RenderData::new(id, rect, UpdateMode::Full));

        let mut file_chooser = FileChooser {
            id,
            rect,
            children,
            current_path: initial_path,
            entries: Vec::new(),
            current_page: 0,
            pages_count: 1,
            mode,
            breadcrumb_index,
            entries_start_index,
            error_message: None,
        };

        file_chooser.navigate_to(file_chooser.current_path.clone(), rq, context);

        file_chooser
    }

    fn list_directory(&self, path: &Path) -> Result<Vec<FileEntryData>, String> {
        let mut entries = Vec::new();

        if !path.exists() {
            return Err("Path does not exist".to_string());
        }

        if !path.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        match fs::read_dir(path) {
            Ok(read_dir) => {
                for entry in read_dir.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        let path = entry.path();

                        if self.mode == SelectionMode::Directory && !metadata.is_dir() {
                            continue;
                        }

                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();

                        let size = if metadata.is_file() {
                            Some(metadata.len())
                        } else {
                            None
                        };

                        let modified = metadata.modified().ok();

                        entries.push(FileEntryData {
                            path,
                            name,
                            size,
                            modified,
                            is_dir: metadata.is_dir(),
                        });
                    }
                }
            }
            Err(err) => {
                return Err(format!("Failed to read directory: {}", err));
            }
        }

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    fn navigate_to(&mut self, path: PathBuf, rq: &mut RenderQueue, context: &mut Context) {
        self.current_path = path;
        match self.list_directory(&self.current_path) {
            Ok(entries) => {
                self.entries = entries;
                self.error_message = None;
            }
            Err(err) => {
                self.entries = Vec::new();
                self.error_message = Some(err);
            }
        }
        self.current_page = 0;

        self.update_breadcrumb(context);
        self.update_entries_list(rq, context);
    }

    fn update_breadcrumb(&mut self, context: &mut Context) {
        let breadcrumb = self.children[self.breadcrumb_index]
            .as_mut()
            .downcast_mut::<Breadcrumb>()
            .unwrap();
        breadcrumb.set_path(&self.current_path, &mut context.fonts);
    }

    fn calculate_entry_rect(
        &self,
        y_pos: i32,
        index: usize,
        max_lines: usize,
        big_height: i32,
        big_thickness: i32,
        small_thickness: i32,
    ) -> Rectangle {
        let y_min = y_pos + if index > 0 { big_thickness } else { 0 };
        let y_max = y_pos + big_height
            - if index < max_lines - 1 {
                small_thickness
            } else {
                0
            };

        rect![self.rect.min.x, y_min, self.rect.max.x, y_max]
    }

    fn add_error_label(&mut self, breadcrumb_bottom: i32, thickness: i32, big_height: i32) {
        if let Some(error_msg) = &self.error_message {
            let label = Label::new(
                rect![
                    self.rect.min.x,
                    breadcrumb_bottom + thickness,
                    self.rect.max.x,
                    breadcrumb_bottom + thickness + big_height * 2
                ],
                format!("Error: {}", error_msg),
                crate::view::Align::Center,
            );
            self.children.push(Box::new(label) as Box<dyn View>);
        }
    }

    fn add_empty_label(&mut self, breadcrumb_bottom: i32, thickness: i32, big_height: i32) {
        let label = Label::new(
            rect![
                self.rect.min.x,
                breadcrumb_bottom + thickness,
                self.rect.max.x,
                breadcrumb_bottom + thickness + big_height
            ],
            "Empty directory".to_string(),
            crate::view::Align::Center,
        );
        self.children.push(Box::new(label) as Box<dyn View>);
    }

    fn add_file_entries(
        &mut self,
        start_idx: usize,
        end_idx: usize,
        breadcrumb_bottom: i32,
        thickness: i32,
        big_height: i32,
        big_thickness: i32,
        small_thickness: i32,
        max_lines: usize,
        context: &mut Context,
    ) {
        let mut y_pos = breadcrumb_bottom + thickness;

        for (i, entry_data) in self.entries[start_idx..end_idx].iter().enumerate() {
            let entry_rect = self.calculate_entry_rect(
                y_pos,
                i,
                max_lines,
                big_height,
                big_thickness,
                small_thickness,
            );

            let file_entry = FileEntry::new(entry_rect, entry_data.clone(), context);
            self.children.push(Box::new(file_entry) as Box<dyn View>);

            let should_add_separator = i < max_lines - 1;
            if should_add_separator {
                let y_max = entry_rect.max.y;
                let separator_rect =
                    rect![self.rect.min.x, y_max, self.rect.max.x, y_max + thickness];
                self.children.push(Self::create_separator(separator_rect));
            }

            y_pos += big_height;
        }
    }

    fn update_entries_list(&mut self, rq: &mut RenderQueue, context: &mut Context) {
        self.children.drain(self.entries_start_index..);

        let layout = FileChooserLayout::new(CURRENT_DEVICE.dpi);
        let breadcrumb_bottom = self.children[self.breadcrumb_index].rect().max.y;
        let available_height =
            self.rect.max.y - breadcrumb_bottom - layout.thickness - layout.small_height;
        let max_lines = (available_height / layout.big_height).max(1) as usize;

        self.pages_count = (self.entries.len() as f32 / max_lines as f32).ceil() as usize;
        if self.pages_count == 0 {
            self.pages_count = 1;
        }

        let start_idx = self.current_page * max_lines;
        let end_idx = (start_idx + max_lines).min(self.entries.len());

        if self.error_message.is_some() {
            self.add_error_label(breadcrumb_bottom, layout.thickness, layout.big_height);
        } else if self.entries.is_empty() {
            if self.mode == SelectionMode::Directory {
                // don't show "Empty directory" when selecting directories
            } else {
                self.add_empty_label(breadcrumb_bottom, layout.thickness, layout.big_height);
            }
        } else {
            self.add_file_entries(
                start_idx,
                end_idx,
                breadcrumb_bottom,
                layout.thickness,
                layout.big_height,
                layout.big_thickness,
                layout.small_thickness,
                max_lines,
                context,
            );
        }

        let separator_rect = rect![
            self.rect.min.x,
            self.rect.max.y - layout.small_height - layout.thickness,
            self.rect.max.x,
            self.rect.max.y - layout.small_height
        ];
        self.children.push(Self::create_separator(separator_rect));

        self.create_bottom_bar();

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Partial));
    }

    fn create_bottom_bar(&mut self) {
        let dpi = CURRENT_DEVICE.dpi;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (_, big_thickness) = halves(thickness);

        let bottom_bar_rect = rect![
            self.rect.min.x,
            self.rect.max.y - small_height + big_thickness,
            self.rect.max.x,
            self.rect.max.y
        ];

        let side = bottom_bar_rect.height() as i32;
        let is_prev_disabled = self.pages_count < 2 || self.current_page == 0;
        let is_next_disabled = self.pages_count < 2 || self.current_page == self.pages_count - 1;

        let prev_rect = rect![bottom_bar_rect.min, bottom_bar_rect.min + side];
        if is_prev_disabled {
            let prev_filler = Filler::new(prev_rect, WHITE);
            self.children.push(Box::new(prev_filler) as Box<dyn View>);
        } else {
            let prev_icon = Icon::new("arrow-left", prev_rect, Event::Page(CycleDir::Previous));
            self.children.push(Box::new(prev_icon) as Box<dyn View>);
        }

        let page_label = PageLabel::new(
            rect![
                bottom_bar_rect.min.x + side,
                bottom_bar_rect.min.y,
                bottom_bar_rect.max.x - side,
                bottom_bar_rect.max.y
            ],
            self.current_page,
            self.pages_count,
            false,
        );
        self.children.push(Box::new(page_label) as Box<dyn View>);

        let next_rect = rect![bottom_bar_rect.max - side, bottom_bar_rect.max];
        if is_next_disabled {
            let next_filler = Filler::new(next_rect, WHITE);
            self.children.push(Box::new(next_filler) as Box<dyn View>);
        } else {
            let next_icon = Icon::new("arrow-right", next_rect, Event::Page(CycleDir::Next));
            self.children.push(Box::new(next_icon) as Box<dyn View>);
        }
    }

    fn select_item(&mut self, path: PathBuf, hub: &Hub) {
        let is_dir = path.is_dir();

        let can_select = match self.mode {
            SelectionMode::File => !is_dir,
            SelectionMode::Directory => is_dir,
            SelectionMode::Both => true,
        };

        // if can_select {
        //     hub.send(Event::FileChooserClosed(Some(path))).ok();
        // }
    }

    fn go_to_page(&mut self, dir: CycleDir, rq: &mut RenderQueue, context: &mut Context) {
        match dir {
            CycleDir::Next => {
                if self.current_page < self.pages_count - 1 {
                    self.current_page += 1;
                }
            }
            CycleDir::Previous => {
                if self.current_page > 0 {
                    self.current_page -= 1;
                }
            }
        }
        self.update_entries_list(rq, context);
    }
}

impl View for FileChooser {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::SelectDirectory(path) => {
                dbg!(&evt);
                self.navigate_to(path.clone(), rq, context);
                true
            }
            Event::Select(EntryId::FileEntry(path)) => {
                dbg!(&evt);
                self.select_item(path.clone(), hub);
                true
            }
            Event::Hold(EntryId::FileEntry(path)) => {
                dbg!(&evt);
                self.select_item(path.clone(), hub);
                true
            }
            Event::Page(dir) => {
                self.go_to_page(*dir, rq, context);
                true
            }
            Event::Close(ViewId::FileChooser) => {
                hub.send(Event::FileChooserClosed(None)).ok();
                true
            }
            _ => false,
        }
    }

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
}
