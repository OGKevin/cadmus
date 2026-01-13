use super::FileEntryData;
use crate::color::{TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::view::{Bus, EntryId, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use chrono::{DateTime, Local};

pub struct FileEntry {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    data: FileEntryData,
}

impl FileEntry {
    pub fn new(rect: Rectangle, data: FileEntryData, _context: &mut Context) -> FileEntry {
        FileEntry {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
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

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;
        fb.draw_rectangle(&self.rect, WHITE);

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let icon = if self.data.is_dir { "📁" } else { "📄" };
        let size_text = self
            .data
            .size
            .map(Self::format_size)
            .unwrap_or_else(|| "-".to_string());
        let date_text = self
            .data
            .modified
            .map(Self::format_date)
            .unwrap_or_else(|| "-".to_string());

        let mut x = self.rect.min.x + padding;
        let y = self.rect.min.y + (self.rect.height() as i32 - x_height) / 2 + x_height;

        let icon_plan = font.plan(icon, None, None);
        font.render(fb, TEXT_NORMAL[1], &icon_plan, pt!(x, y));
        x += icon_plan.width + padding;

        let date_plan = font.plan(&date_text, None, None);
        let size_plan = font.plan(&size_text, None, None);

        let name_max_width = self.rect.width() as i32 - x + self.rect.min.x
            - date_plan.width
            - size_plan.width
            - 4 * padding;

        let name_plan = font.plan(&self.data.name, Some(name_max_width), None);
        font.render(fb, TEXT_NORMAL[1], &name_plan, pt!(x, y));

        let size_x = self.rect.max.x - date_plan.width - size_plan.width - 2 * padding;
        font.render(fb, TEXT_NORMAL[1], &size_plan, pt!(size_x, y));

        let date_x = self.rect.max.x - date_plan.width - padding;
        font.render(fb, TEXT_NORMAL[1], &date_plan, pt!(date_x, y));
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
