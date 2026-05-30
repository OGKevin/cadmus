mod file_entry;

pub use self::file_entry::FileEntry;

use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{CycleDir, Rectangle, halves};
use crate::gesture::GestureEvent;
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::label::Label;
use crate::view::navigation::StackNavigationBar;
use crate::view::navigation::providers::directory::DirectoryNavigationProvider;
use crate::view::page_label::PageLabel;
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{Bus, EntryId, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, View, ViewId};
use crate::view::{SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The text displayed for the "Select Current Folder" entry
const SELECT_CURRENT_FOLDER_TEXT: &str = "Select this folder";

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
        let big_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

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

    fn nav_bar_rect(&self, rect: &Rectangle) -> Rectangle {
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
    nav_bar_index: usize,
    entries_start_index: usize,
    error_message: Option<String>,

    /// The path that was selected by the user.
    /// This is used to determine how the file chooser should be closed.
    selected_path: Option<PathBuf>,

    bottom_bar_rect: Rectangle,
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

    #[inline]
    fn build_children(
        rect: Rectangle,
        initial_path: &Path,
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
            TopBarVariant::Cancel(Event::Close(ViewId::FileChooser)),
            title.to_string(),
            context,
        );
        children.push(Box::new(top_bar) as Box<dyn View>);

        children.push(Self::create_separator(layout.first_separator_rect(&rect)));

        let nav_bar_index = children.len();
        let provider = DirectoryNavigationProvider::filesystem(PathBuf::from("/"));
        let nav_bar = StackNavigationBar::new(
            layout.nav_bar_rect(&rect),
            rect.max.y - layout.small_height - layout.thickness,
            3,
            provider,
            initial_path.to_path_buf(),
        );

        children.push(Box::new(nav_bar) as Box<dyn View>);
        children.push(Self::create_separator(layout.second_separator_rect(&rect)));

        (children, nav_bar_index)
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

        let (children, nav_bar_index) =
            Self::build_children(rect, &initial_path, mode, &layout, context);
        let entries_start_index = children.len();

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));

        let mut file_chooser = FileChooser {
            id,
            rect,
            children,
            current_path: initial_path.clone(),
            entries: Vec::new(),
            current_page: 0,
            pages_count: 1,
            mode,
            nav_bar_index,
            entries_start_index,
            error_message: None,
            selected_path: None,
            bottom_bar_rect: Rectangle::default(),
        };

        file_chooser.navigate_to(initial_path, rq, context);

        file_chooser
    }

    /// Lists files in the given directory.
    ///
    /// In Directory mode, returns an empty list since directories are navigated
    /// via the navigation bar and only the "Select Current Folder" special entry
    /// is shown in the content area.
    fn list_directory(&self, path: &Path) -> Result<Vec<FileEntryData>, String> {
        let mut entries = Vec::new();

        if !path.exists() {
            return Err("Path does not exist".to_string());
        }

        if !path.is_dir() {
            return Err("Path is not a directory".to_string());
        }

        if self.mode == SelectionMode::Directory {
            return Ok(entries);
        }

        match fs::read_dir(path) {
            Ok(read_dir) => {
                for entry in read_dir.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_dir() {
                            continue;
                        }

                        let path = entry.path();

                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();

                        let size = Some(metadata.len());
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

        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

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

        if self.error_message.is_none() {
            if let Some(select_current_entry) = self.create_select_current_entry() {
                self.entries.insert(0, select_current_entry);
            }
        }

        self.current_page = 0;

        let nav_bar = self.children[self.nav_bar_index]
            .as_mut()
            .downcast_mut::<StackNavigationBar<DirectoryNavigationProvider>>()
            .unwrap();
        nav_bar.set_selected(self.current_path.clone(), rq, context);

        self.update_nav_bar_separator();
        self.update_entries_list(context);

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    #[inline]
    fn update_nav_bar_separator(&mut self) {
        let nav_bar_bottom = self.children[self.nav_bar_index].rect().max.y;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, CURRENT_DEVICE.dpi) as i32;
        let separator_rect = rect![
            self.rect.min.x,
            nav_bar_bottom,
            self.rect.max.x,
            nav_bar_bottom + thickness
        ];

        if let Some(separator) = self.children[self.nav_bar_index + 1]
            .as_mut()
            .downcast_mut::<Filler>()
        {
            *separator.rect_mut() = separator_rect;
        }
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

    fn add_error_label(&mut self, nav_bar_bottom: i32, thickness: i32, big_height: i32) {
        if let Some(error_msg) = &self.error_message {
            let label = Label::new(
                rect![
                    self.rect.min.x,
                    nav_bar_bottom + thickness,
                    self.rect.max.x,
                    nav_bar_bottom + thickness + big_height * 2
                ],
                format!("Error: {}", error_msg),
                crate::view::Align::Center,
            );
            self.children.push(Box::new(label) as Box<dyn View>);
        }
    }

    fn add_empty_label(&mut self, nav_bar_bottom: i32, thickness: i32, big_height: i32) {
        let label = Label::new(
            rect![
                self.rect.min.x,
                nav_bar_bottom + thickness,
                self.rect.max.x,
                nav_bar_bottom + thickness + big_height
            ],
            "Empty directory".to_string(),
            crate::view::Align::Center,
        );
        self.children.push(Box::new(label) as Box<dyn View>);
    }

    #[allow(clippy::too_many_arguments)]
    /// Adds file entry views to the FileChooser's children for the current page.
    ///
    /// Each file entry is represented using the `FileEntry` component, which displays
    /// the file or directory's name, icon, and metadata (such as size and modification date).
    /// Between each entry, a separator is added using the `Filler` component to visually
    /// separate the entries.
    ///
    /// Components used to build each file entry:
    /// - [`FileEntry`]: Displays the file or directory entry, including icon, name, and metadata.
    /// - [`Filler`]: Used as a separator between file entries for visual clarity.
    ///
    /// # Arguments
    /// * `start_idx` - The starting index of the entries to display.
    /// * `end_idx` - The ending index (exclusive) of the entries to display.
    /// * `nav_bar_bottom` - The y-coordinate below the breadcrumb bar.
    /// * `thickness` - The thickness of the separator lines.
    /// * `big_height` - The height of each file entry row.
    /// * `big_thickness` - The thickness of the separator between entries.
    /// * `small_thickness` - The thickness of the separator at the end of the list.
    /// * `max_lines` - The maximum number of entries to display per page.
    /// * `context`
    fn add_file_entries(
        &mut self,
        start_idx: usize,
        end_idx: usize,
        nav_bar_bottom: i32,
        thickness: i32,
        big_height: i32,
        big_thickness: i32,
        small_thickness: i32,
        max_lines: usize,
        context: &mut Context,
    ) {
        let mut y_pos = nav_bar_bottom + thickness;

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

            let y_max = entry_rect.max.y;
            let separator_rect = rect![self.rect.min.x, y_max, self.rect.max.x, y_max + thickness];
            self.children.push(Self::create_separator(separator_rect));

            y_pos += big_height;
        }
    }

    fn update_entries_list(&mut self, context: &mut Context) {
        self.children.drain(self.entries_start_index..);

        let layout = FileChooserLayout::new(CURRENT_DEVICE.dpi);
        let nav_bar_bottom = self.children[self.nav_bar_index].rect().max.y;
        let available_height =
            self.rect.max.y - nav_bar_bottom - layout.thickness - layout.small_height;
        let max_lines = (available_height / layout.big_height).max(1) as usize;

        self.pages_count = (self.entries.len() as f32 / max_lines as f32).ceil() as usize;
        if self.pages_count == 0 {
            self.pages_count = 1;
        }

        let start_idx = self.current_page * max_lines;
        let end_idx = (start_idx + max_lines).min(self.entries.len());

        if self.error_message.is_some() {
            self.add_error_label(nav_bar_bottom, layout.thickness, layout.big_height);
        } else if self.entries.is_empty() {
            self.add_empty_label(nav_bar_bottom, layout.thickness, layout.big_height);
        } else {
            self.add_file_entries(
                start_idx,
                end_idx,
                nav_bar_bottom,
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

        self.bottom_bar_rect = bottom_bar_rect;

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

    /// Creates a special "Select Current Folder" entry when in Directory or Both mode.
    /// This entry allows the user to select the current directory rather than navigating into it.
    #[inline]
    fn create_select_current_entry(&self) -> Option<FileEntryData> {
        match self.mode {
            SelectionMode::File => None,
            SelectionMode::Directory | SelectionMode::Both => Some(FileEntryData {
                path: self.current_path.clone(),
                name: SELECT_CURRENT_FOLDER_TEXT.to_string(),
                size: None,
                modified: None,
                is_dir: true,
            }),
        }
    }

    /// Selects the given item if it matches the selection mode.
    /// Sends FileChooserClosed event with the selected path to the bus.
    fn select_item(&mut self, path: PathBuf, bus: &mut Bus) {
        let is_dir = path.is_dir();

        let can_select = match self.mode {
            SelectionMode::File => !is_dir,
            SelectionMode::Directory => is_dir,
            SelectionMode::Both => true,
        };

        if can_select {
            self.selected_path = Some(path);
            bus.push_back(Event::FileChooserClosed(self.selected_path.clone()));
            bus.push_back(Event::Close(self.view_id().unwrap()));
        }
    }

    fn go_to_page(&mut self, dir: CycleDir, context: &mut Context) {
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
        self.update_entries_list(context);
    }
}

impl View for FileChooser {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::ToggleSelectDirectory(path) => {
                self.navigate_to(path.clone(), rq, context);
                true
            }
            Event::Select(EntryId::FileEntry(path)) => {
                self.select_item(path.clone(), bus);
                true
            }
            Event::Page(dir) => {
                self.go_to_page(*dir, context);

                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

                true
            }
            Event::NavigationBarResized(_) => {
                self.update_nav_bar_separator();
                self.update_entries_list(context);

                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));

                true
            }
            Event::Gesture(GestureEvent::Tap(center)) if self.bottom_bar_rect.includes(*center) => {
                true
            }
            _ => false,
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
        Some(ViewId::FileChooser)
    }
}

#[cfg(test)]
impl FileChooser {
    pub fn bottom_bar_rect(&self) -> Rectangle {
        self.bottom_bar_rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_test_file_chooser(rq: &mut RenderQueue, context: &mut Context) -> FileChooser {
        let rect = rect![0, 0, 600, 800];
        let path = PathBuf::from("/tmp");
        let (hub, _receiver) = channel();
        FileChooser::new(rect, path, SelectionMode::File, &hub, rq, context)
    }

    fn create_test_file_chooser_with_path(
        rq: &mut RenderQueue,
        context: &mut Context,
        path: PathBuf,
        mode: SelectionMode,
    ) -> FileChooser {
        let rect = rect![0, 0, 600, 800];
        let (hub, _receiver) = channel();
        FileChooser::new(rect, path, mode, &hub, rq, context)
    }

    #[test]
    fn test_bottom_bar_rect_stored_correctly() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let bottom_bar = file_chooser.bottom_bar_rect();

        assert!(
            bottom_bar.max.y > 0,
            "bottom_bar_rect should be properly initialized"
        );
        assert_eq!(
            bottom_bar.min.x, 0,
            "bottom_bar_rect should start at left edge"
        );
        assert_eq!(
            bottom_bar.max.x, 600,
            "bottom_bar_rect should span full width"
        );
        assert!(
            bottom_bar.min.y < bottom_bar.max.y,
            "bottom_bar_rect should have positive height"
        );
    }

    #[test]
    fn test_tap_in_bottom_bar_is_consumed() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let bottom_bar = file_chooser.bottom_bar_rect();
        let center = Point {
            x: (bottom_bar.min.x + bottom_bar.max.x) / 2,
            y: (bottom_bar.min.y + bottom_bar.max.y) / 2,
        };

        let tap_event = Event::Gesture(GestureEvent::Tap(center));
        let consumed = file_chooser.handle_event(&tap_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Tap event in bottom bar should be consumed");
        assert!(
            bus.is_empty(),
            "Consumed event should not be forwarded to bus"
        );
    }

    #[test]
    fn test_tap_outside_bottom_bar_not_consumed() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let bottom_bar = file_chooser.bottom_bar_rect();
        let entry_point = Point {
            x: 300,
            y: bottom_bar.min.y - 50,
        };

        let tap_event = Event::Gesture(GestureEvent::Tap(entry_point));
        let consumed = file_chooser.handle_event(&tap_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            !consumed,
            "Tap event outside bottom bar should not be consumed"
        );
    }

    #[test]
    fn test_page_event_still_handled() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let page_event = Event::Page(CycleDir::Next);
        let consumed =
            file_chooser.handle_event(&page_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Page event should still be handled correctly");
    }

    #[test]
    fn test_tap_on_bottom_bar_edge_is_consumed() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let bottom_bar = file_chooser.bottom_bar_rect();
        let edge_point = Point {
            x: bottom_bar.min.x + 1,
            y: bottom_bar.min.y + 1,
        };

        let tap_event = Event::Gesture(GestureEvent::Tap(edge_point));
        let consumed = file_chooser.handle_event(&tap_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Tap event on bottom bar edge should be consumed");
    }

    // Tests for list_directory returning only files

    #[test]
    fn test_list_directory_returns_only_files() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        fs::write(temp_path.join("alpha.txt"), "content").unwrap();
        fs::write(temp_path.join("beta.txt"), "content").unwrap();
        fs::write(temp_path.join("gamma.txt"), "content").unwrap();

        fs::create_dir(temp_path.join("subdir1")).unwrap();
        fs::create_dir(temp_path.join("subdir2")).unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let entries = file_chooser.list_directory(temp_path).unwrap();

        assert_eq!(
            entries.len(),
            3,
            "Should only return files, not directories"
        );
        for entry in &entries {
            assert!(
                !entry.is_dir,
                "Entry {} should not be a directory",
                entry.name
            );
        }

        assert_eq!(entries[0].name, "alpha.txt");
        assert_eq!(entries[1].name, "beta.txt");
        assert_eq!(entries[2].name, "gamma.txt");
    }

    #[test]
    fn test_list_directory_returns_empty_for_empty_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let entries = file_chooser.list_directory(temp_path).unwrap();

        assert!(
            entries.is_empty(),
            "Empty directory should return empty list"
        );
    }

    #[test]
    fn test_list_directory_returns_error_for_nonexistent_path() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let result =
            file_chooser.list_directory(Path::new("/nonexistent/path/that/does/not/exist"));

        assert!(result.is_err(), "Should return error for nonexistent path");
        assert!(
            result.unwrap_err().contains("does not exist"),
            "Error should mention path does not exist"
        );
    }

    #[test]
    fn test_list_directory_returns_error_for_file_path() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();
        let file_path = temp_path.join("test_file.txt");
        fs::write(&file_path, "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let result = file_chooser.list_directory(&file_path);

        assert!(result.is_err(), "Should return error when path is a file");
        assert!(
            result.unwrap_err().contains("not a directory"),
            "Error should mention path is not a directory"
        );
    }

    #[test]
    fn test_list_directory_sorts_alphabetically_case_insensitive() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        fs::write(temp_path.join("Zebra.txt"), "content").unwrap();
        fs::write(temp_path.join("alpha.txt"), "content").unwrap();
        fs::write(temp_path.join("BETA.txt"), "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let entries = file_chooser.list_directory(temp_path).unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "alpha.txt");
        assert_eq!(entries[1].name, "BETA.txt");
        assert_eq!(entries[2].name, "Zebra.txt");
    }

    #[test]
    fn test_list_directory_returns_no_files_in_directory_mode() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        fs::write(temp_path.join("file1.txt"), "content").unwrap();
        fs::write(temp_path.join("file2.txt"), "content").unwrap();
        fs::create_dir(temp_path.join("subdir")).unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        let entries = file_chooser.list_directory(temp_path).unwrap();

        assert_eq!(
            entries.len(),
            0,
            "Directory mode should return no files - only navigation bar shows directories"
        );
    }

    // Tests for "Select Current Folder" entry

    #[test]
    fn test_select_current_folder_entry_in_directory_mode() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        assert!(
            !file_chooser.entries.is_empty(),
            "Should have at least one entry (Select Current Folder)"
        );
        assert_eq!(
            file_chooser.entries[0].name, SELECT_CURRENT_FOLDER_TEXT,
            "Select Current Folder entry should be at index 0"
        );
        assert!(
            file_chooser.entries[0].is_dir,
            "Select Current Folder entry should be marked as directory"
        );
    }

    #[test]
    fn test_select_current_folder_entry_in_both_mode() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Both,
        );

        assert!(
            !file_chooser.entries.is_empty(),
            "Should have at least one entry (Select Current Folder)"
        );
        assert_eq!(
            file_chooser.entries[0].name, SELECT_CURRENT_FOLDER_TEXT,
            "Select Current Folder entry should be at index 0"
        );
    }

    #[test]
    fn test_no_select_current_folder_entry_in_file_mode() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        fs::write(temp_path.join("test.txt"), "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        for entry in &file_chooser.entries {
            assert_ne!(
                entry.name, SELECT_CURRENT_FOLDER_TEXT,
                "File mode should not contain Select Current Folder text"
            );
        }
    }

    #[test]
    fn test_select_current_folder_entry_path_is_current_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        assert_eq!(
            file_chooser.entries[0].path, temp_path,
            "Select Current Folder entry should point to current directory"
        );
    }

    // Tests for selecting the current folder

    #[test]
    fn test_select_current_folder_selects_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(temp_path.to_path_buf()));
        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            consumed,
            "Select event for current folder should be consumed"
        );

        let mut found_close_event = false;
        let mut found_file_chooser_closed = false;

        for event in &bus {
            match event {
                Event::FileChooserClosed(Some(path)) => {
                    found_file_chooser_closed = true;
                    assert_eq!(
                        path, &temp_path,
                        "FileChooserClosed should contain the selected directory path"
                    );
                }
                Event::Close(ViewId::FileChooser) => {
                    found_close_event = true;
                }
                _ => {}
            }
        }

        assert!(
            found_file_chooser_closed,
            "FileChooserClosed event should be in bus"
        );
        assert!(
            found_close_event,
            "Close FileChooser event should be in bus"
        );
    }

    #[test]
    fn test_select_current_folder_in_both_mode() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Both,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(temp_path.to_path_buf()));
        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            consumed,
            "Select event for current folder should be consumed in Both mode"
        );

        let found_close = bus
            .iter()
            .any(|e| matches!(e, Event::Close(ViewId::FileChooser)));
        assert!(
            found_close,
            "Close event should be sent when selecting current folder in Both mode"
        );
    }

    // Tests for mode-specific selection behavior

    #[test]
    fn test_file_mode_rejects_directory_selection() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let subdir_path = temp_path.join("subdir");
        fs::create_dir(&subdir_path).unwrap();
        let select_event = Event::Select(EntryId::FileEntry(subdir_path));

        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Select event should be consumed");
        assert!(
            bus.is_empty(),
            "No events should be sent when rejecting directory in File mode"
        );
    }

    #[test]
    fn test_directory_mode_accepts_directory_selection() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(temp_path.to_path_buf()));
        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Select event should be consumed");

        let found_close = bus
            .iter()
            .any(|e| matches!(e, Event::Close(ViewId::FileChooser)));
        assert!(
            found_close,
            "Close event should be sent when selecting directory in Directory mode"
        );
    }

    #[test]
    fn test_directory_mode_rejects_file_selection() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let file_path = temp_path.join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Directory,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(file_path));

        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Select event should be consumed");
        assert!(
            bus.is_empty(),
            "No events should be sent when rejecting file in Directory mode"
        );
    }

    #[test]
    fn test_both_mode_accepts_file_selection() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let file_path = temp_path.join("test.txt");
        fs::write(&file_path, "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Both,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(file_path.clone()));
        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Select event should be consumed");

        let found_file_chooser_closed = bus
            .iter()
            .any(|e| matches!(e, Event::FileChooserClosed(Some(path)) if path == &file_path));
        assert!(
            found_file_chooser_closed,
            "FileChooserClosed should be sent with file path in Both mode"
        );
    }

    #[test]
    fn test_both_mode_accepts_directory_selection() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::Both,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let select_event = Event::Select(EntryId::FileEntry(temp_path.to_path_buf()));
        let consumed =
            file_chooser.handle_event(&select_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "Select event should be consumed");

        let found_file_chooser_closed = bus
            .iter()
            .any(|e| matches!(e, Event::FileChooserClosed(Some(path)) if path == temp_path));
        assert!(
            found_file_chooser_closed,
            "FileChooserClosed should be sent with directory path in Both mode"
        );
    }

    // Tests for navigation bar integration

    #[test]
    fn test_navigation_bar_is_initialized() {
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let file_chooser = create_test_file_chooser(&mut rq, &mut context);

        let nav_bar_child = file_chooser.children.get(file_chooser.nav_bar_index);
        assert!(
            nav_bar_child.is_some(),
            "Navigation bar should be present at nav_bar_index"
        );

        let can_downcast = nav_bar_child
            .unwrap()
            .as_any()
            .downcast_ref::<StackNavigationBar<DirectoryNavigationProvider>>()
            .is_some();
        assert!(
            can_downcast,
            "Child at nav_bar_index should be StackNavigationBar"
        );
    }

    #[test]
    fn test_navigate_to_updates_navigation_bar() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let initial_path = file_chooser.current_path.clone();

        let subdir_path = temp_path.join("subdir");
        fs::create_dir(&subdir_path).unwrap();

        file_chooser.navigate_to(subdir_path.clone(), &mut rq, &mut context);

        assert_eq!(
            file_chooser.current_path, subdir_path,
            "Current path should be updated after navigation"
        );
        assert_ne!(
            file_chooser.current_path, initial_path,
            "Current path should have changed"
        );
    }

    #[test]
    fn test_toggle_select_directory_event_navigates() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        let subdir_path = temp_path.join("navtarget");
        fs::create_dir(&subdir_path).unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let initial_path = file_chooser.current_path.clone();

        let toggle_event = Event::ToggleSelectDirectory(subdir_path.clone());
        let consumed =
            file_chooser.handle_event(&toggle_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(consumed, "ToggleSelectDirectory event should be consumed");
        assert_eq!(
            file_chooser.current_path, subdir_path,
            "Current path should be updated to subdirectory"
        );
        assert_ne!(
            file_chooser.current_path, initial_path,
            "Current path should have changed"
        );
    }

    #[test]
    fn test_navigation_bar_resized_event_consumed() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp directory");
        let temp_path = temp_dir.path();

        fs::write(temp_path.join("test.txt"), "content").unwrap();

        let mut rq = RenderQueue::new();
        let mut context = create_test_context();
        let mut file_chooser = create_test_file_chooser_with_path(
            &mut rq,
            &mut context,
            temp_path.to_path_buf(),
            SelectionMode::File,
        );

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();

        let resized_event = Event::NavigationBarResized(50);
        let consumed =
            file_chooser.handle_event(&resized_event, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            consumed,
            "NavigationBarResized event should be consumed by FileChooser"
        );
    }
}
