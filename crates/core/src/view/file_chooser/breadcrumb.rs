use crate::color::{TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::unit::scale_by_dpi;
use crate::view::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use std::path::{Path, PathBuf};

pub struct Breadcrumb {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    path: PathBuf,
}

struct BreadcrumbEntry {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    path: Option<PathBuf>,
    text: String,
    is_current: bool,
}

impl BreadcrumbEntry {
    fn new(rect: Rectangle, path: Option<PathBuf>, text: String, is_current: bool) -> Self {
        BreadcrumbEntry {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            path,
            text,
            is_current,
        }
    }
}

impl View for BreadcrumbEntry {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Gesture(GestureEvent::Tap(center))
                if self.rect.includes(*center) && !self.is_current =>
            {
                match &self.path {
                    Some(p) => bus.push_back(Event::SelectDirectory(p.clone())),
                    None => (),
                }
                true
            }
            _ => false,
        }
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);

        let plan = font.plan(&self.text, None, None);
        let dx = (self.rect.width() as i32 - plan.width as i32) / 2;
        let dy = (self.rect.height() as i32 - font.x_heights.0 as i32) / 2;
        let pt = pt!(self.rect.min.x + dx, self.rect.max.y - dy);

        font.render(fb, TEXT_NORMAL[1], &plan, pt);
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

struct ComponentData {
    path: PathBuf,
    text: String,
    width: i32,
    is_current: bool,
}

impl Breadcrumb {
    pub fn new(rect: Rectangle, path: &Path) -> Breadcrumb {
        let id = ID_FEEDER.next();
        let children = Vec::new();
        Breadcrumb {
            id,
            rect,
            children,
            path: path.to_path_buf(),
        }
    }

    fn build_path_components(path: &Path) -> Vec<PathBuf> {
        let mut components: Vec<PathBuf> = Vec::new();
        let mut current = path;

        while let Some(parent) = current.parent() {
            components.push(current.to_path_buf());
            current = parent;
        }
        components.push(current.to_path_buf());
        components.reverse();
        components
    }

    fn create_component_data(
        components: &[PathBuf],
        font: &mut crate::font::Font,
    ) -> Vec<ComponentData> {
        let mut component_data: Vec<ComponentData> = Vec::new();

        for (i, component_path) in components.iter().enumerate() {
            let name = component_path
                .file_name()
                .unwrap_or_else(|| {
                    if component_path.as_os_str() == "/" {
                        std::ffi::OsStr::new("/")
                    } else {
                        component_path.as_os_str()
                    }
                })
                .to_string_lossy()
                .to_string();

            let text = if i == components.len() - 1 {
                name.clone()
            } else if name == "/" {
                "/ ".to_string()
            } else {
                format!("{} / ", name)
            };

            let width = font.plan(&text, None, None).width as i32;
            let is_current = i == components.len() - 1;

            component_data.push(ComponentData {
                path: component_path.clone(),
                text,
                width,
                is_current,
            });
        }

        component_data
    }

    fn calculate_start_index(
        component_data: &[ComponentData],
        available_width: i32,
        font: &mut crate::font::Font,
    ) -> usize {
        let total_width: i32 = component_data.iter().map(|c| c.width).sum();

        if total_width <= available_width {
            return 0;
        }

        let ellipsis_text = "... / ";
        let ellipsis_width = font.plan(ellipsis_text, None, None).width as i32;

        let mut accumulated_width = ellipsis_width;
        let mut start_idx = component_data.len();

        for i in (0..component_data.len()).rev() {
            if accumulated_width + component_data[i].width > available_width {
                break;
            }
            accumulated_width += component_data[i].width;
            start_idx = i;
        }

        start_idx
    }

    fn add_ellipsis_entry(&mut self, ellipsis_width: i32, padding: i32) {
        let ellipsis_rect = rect![
            self.rect.min.x + padding,
            self.rect.min.y,
            self.rect.min.x + padding + ellipsis_width,
            self.rect.max.y
        ];

        let ellipsis_entry = BreadcrumbEntry::new(ellipsis_rect, None, "... / ".to_string(), false);

        self.children
            .push(Box::new(ellipsis_entry) as Box<dyn View>);
    }

    fn create_breadcrumb_entries(
        &mut self,
        component_data: &[ComponentData],
        start_index: usize,
        padding: i32,
        font: &mut crate::font::Font,
    ) {
        let mut x = self.rect.min.x + padding;

        if start_index > 0 {
            let ellipsis_text = "... / ";
            x += font.plan(ellipsis_text, None, None).width as i32;
        }

        for data in component_data.iter().skip(start_index) {
            let segment_rect = rect![x, self.rect.min.y, x + data.width, self.rect.max.y];

            let entry = BreadcrumbEntry::new(
                segment_rect,
                Some(data.path.clone()),
                data.text.clone(),
                data.is_current,
            );

            self.children.push(Box::new(entry) as Box<dyn View>);

            x += data.width;
        }
    }

    pub fn set_path(&mut self, path: &Path, fonts: &mut Fonts) {
        self.path = path.to_path_buf();
        self.children.clear();

        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let padding = scale_by_dpi(8.0, dpi) as i32;

        let components = Self::build_path_components(path);
        let component_data = Self::create_component_data(&components, font);

        let available_width = self.rect.width() as i32 - 2 * padding;
        let start_index = Self::calculate_start_index(&component_data, available_width, font);

        if start_index > 0 {
            let ellipsis_width = font.plan("... / ", None, None).width as i32;
            self.add_ellipsis_entry(ellipsis_width, padding);
        }

        self.create_breadcrumb_entries(&component_data, start_index, padding, font);
    }
}

impl View for Breadcrumb {
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        false
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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
