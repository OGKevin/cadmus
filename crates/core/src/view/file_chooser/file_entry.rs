use super::FileEntryData;
use crate::color::{TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::view::label::Label;
use crate::view::{Align, Bus, EntryId, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use chrono::{DateTime, Local};

/// A visual entry representing a file or directory in the file browser.
///
/// `FileEntry` displays file metadata in a horizontal layout with an icon, name, size, and date.
/// It handles user interactions such as taps to select files and long presses to perform actions
/// on directories.
///
/// # Fields
///
/// * `id` - Unique identifier for this view
/// * `rect` - Bounding rectangle for the entire entry
/// * `children` - Child views (labels for icon, name, size, and date)
/// * `data` - File entry data containing metadata (name, size, date, directory flag, path)
pub struct FileEntry {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    data: FileEntryData,
}

impl FileEntry {
    /// Creates a new file entry with a horizontal layout displaying file metadata.
    ///
    /// # Layout
    ///
    /// The entry displays file information in a left-to-right layout:
    /// - **Icon** (left): Directory folder (📁) or file (📄) emoji
    /// - **Name** (center-left): File or directory name, truncated if necessary
    /// - **Size** (center-right): Formatted file size (e.g., "1.5 MB") or "-" if unavailable
    /// - **Date** (right): Last modified date in format "Mon DD, YYYY HH:MM" or "-" if unavailable
    ///
    /// Each element is separated by padding based on the font's em size.
    /// The name field expands to fill available space between icon and size/date fields.
    ///
    /// # Arguments
    ///
    /// * `rect` - The bounding rectangle for the entire entry
    /// * `data` - The file entry data containing name, size, modification date, and directory flag
    /// * `context` - Mutable reference to the application context for font access
    pub fn new(rect: Rectangle, data: FileEntryData, context: &mut Context) -> FileEntry {
        let mut children: Vec<Box<dyn View>> = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let padding = font.em() as i32;

        let event = if data.is_dir {
            Some(Event::SelectDirectory(data.path.clone()))
        } else {
            Some(Event::Select(EntryId::FileEntry(data.path.clone())))
        };
        let hold_event = if data.is_dir {
            Some(Event::Hold(EntryId::FileEntry(data.path.clone())))
        } else {
            None
        };

        let icon = if data.is_dir { "📁" } else { "📄" };
        let size_text = data
            .size
            .map(Self::format_size)
            .unwrap_or_else(|| "-".to_string());
        let date_text = data
            .modified
            .map(Self::format_date)
            .unwrap_or_else(|| "-".to_string());

        let icon_plan = font.plan(icon, None, None);
        let date_plan = font.plan(&date_text, None, None);
        let size_plan = font.plan(&size_text, None, None);

        let mut x = rect.min.x + padding;
        let icon_width = icon_plan.width + padding;

        let name_max_width = rect.width() as i32
            - icon_width
            - padding
            - date_plan.width
            - size_plan.width
            - 4 * padding;

        let name_plan = font.plan(&data.name, Some(name_max_width), None);

        let icon_rect = rect![x, rect.min.y, x + icon_width, rect.max.y];
        children.push(Box::new(
            Label::new(icon_rect, icon.to_string(), Align::Left(0))
                .scheme([WHITE, TEXT_NORMAL[1], TEXT_NORMAL[2]])
                .event(event.clone())
                .hold_event(hold_event.clone()),
        ));
        x += icon_width;

        let name_rect = rect![x, rect.min.y, x + name_plan.width + padding, rect.max.y];
        children.push(Box::new(
            Label::new(name_rect, data.name.clone(), Align::Left(0))
                .scheme([WHITE, TEXT_NORMAL[1], TEXT_NORMAL[2]])
                .event(event.clone())
                .hold_event(hold_event.clone()),
        ));

        let size_x = rect.max.x - date_plan.width - size_plan.width - 2 * padding;
        let size_rect = rect![
            size_x,
            rect.min.y,
            size_x + size_plan.width + padding,
            rect.max.y
        ];
        children.push(Box::new(
            Label::new(size_rect, size_text, Align::Left(0))
                .scheme([WHITE, TEXT_NORMAL[1], TEXT_NORMAL[2]])
                .event(event.clone())
                .hold_event(hold_event.clone()),
        ));

        let date_x = rect.max.x - date_plan.width - padding;
        let date_rect = rect![date_x, rect.min.y, rect.max.x, rect.max.y];
        children.push(Box::new(
            Label::new(date_rect, date_text, Align::Left(0))
                .scheme([WHITE, TEXT_NORMAL[1], TEXT_NORMAL[2]])
                .event(event.clone())
                .hold_event(hold_event.clone()),
        ));

        FileEntry {
            id: ID_FEEDER.next(),
            rect,
            children,
            data,
        }
    }

    fn format_size(size: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if size >= GB {
            format!("{:.1} GB", size as f64 / GB as f64)
        } else if size >= MB {
            format!("{:.1} MB", size as f64 / MB as f64)
        } else if size >= KB {
            format!("{:.1} KB", size as f64 / KB as f64)
        } else {
            format!("{} B", size)
        }
    }

    fn format_date(system_time: std::time::SystemTime) -> String {
        let datetime: DateTime<Local> = system_time.into();
        datetime.format("%b %d, %Y %H:%M").to_string()
    }
}

impl View for FileEntry {
    /// Handles events for the file entry.
    ///
    /// This method processes user interactions with the file entry:
    /// - **Tap gesture**: If the tap is within the entry's bounds, it pushes either a
    ///   `SelectDirectory` event (for directories) or a `Select` event (for files) to the bus.
    /// - **Hold gesture** (short): If the hold is within the entry's bounds and the entry
    ///   represents a directory, it pushes a `Hold` event to the bus.
    /// - **Other events**: Returns `false` and does not process other event types.
    ///
    /// # Arguments
    ///
    /// * `evt` - The event to handle
    /// * `_hub` - Unused hub reference
    /// * `bus` - The event bus to push generated events to
    /// * `_rq` - Unused render queue reference
    /// * `_context` - Unused context reference
    ///
    /// # Returns
    ///
    /// `true` if the event was handled, `false` otherwise.
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(*center) => {
                if self.data.is_dir {
                    bus.push_back(Event::SelectDirectory(self.data.path.clone()));
                } else {
                    bus.push_back(Event::Select(EntryId::FileEntry(self.data.path.clone())));
                }
                true
            }
            Event::Gesture(GestureEvent::HoldFingerShort(center, _id))
                if self.rect.includes(*center) && self.data.is_dir =>
            {
                bus.push_back(Event::Hold(EntryId::FileEntry(self.data.path.clone())));
                true
            }
            _ => false,
        }
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {
        fb.draw_rectangle(&self.rect, WHITE);
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
