use super::category::Category;
use super::category_button::CategoryButton;
use crate::color::TEXT_BUMP_SMALL;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{Point, Rectangle};
use crate::view::filler::Filler;
use crate::view::{Align, Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};

/// Horizontal navigation bar displaying category tabs.
///
/// This component shows all available settings categories (General, Libraries,
/// Intermissions) as horizontal tabs. The selected category is visually highlighted
/// using `ActionLabel` children that manage their own color states.
///
/// # Structure
///
/// ```text
/// ┌─────────────────────────────────────────────┐
/// │ [General] [Libraries] [Intermissions]       │
/// └─────────────────────────────────────────────┘
/// ```
pub struct CategoryNavigationBar {
    id: Id,
    pub rect: Rectangle,
    children: Vec<Box<dyn View>>,
    pub selected: Category,
}

impl CategoryNavigationBar {
    #[cfg_attr(feature = "otel", tracing::instrument())]
    pub fn new(rect: Rectangle, selected: Category) -> Self {
        let id = ID_FEEDER.next();

        CategoryNavigationBar {
            id,
            rect,
            children: Vec::new(),
            selected,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fonts)))]
    pub fn update_content(&mut self, selected: Category, fonts: &mut Fonts) {
        self.selected = selected;
        self.children.clear();
        self.children = Self::build_category_buttons(self.rect, selected, fonts);
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(fonts)))]
    fn build_category_buttons(
        rect: Rectangle,
        selected: Category,
        fonts: &mut Fonts,
    ) -> Vec<Box<dyn View>> {
        let mut children = Vec::new();
        let categories = Category::all();
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let padding = font.em() as i32;
        let background = TEXT_BUMP_SMALL[0];

        let mut x_pos = rect.min.x + padding / 2;

        for category in categories.iter() {
            let text = category.label();
            let plan = font.plan(&text, None, None);
            let button_width = plan.width + padding;

            let button_rect = rect![x_pos, rect.min.y, x_pos + button_width, rect.max.y];
            let is_selected = *category == selected;

            let button = CategoryButton::new(
                button_rect,
                *category,
                is_selected,
                Align::Left(padding / 2),
            );
            children.push(Box::new(button) as Box<dyn View>);

            x_pos += button_width;
        }

        if x_pos < rect.max.x {
            let filler_rect = rect![x_pos, rect.min.y, rect.max.x, rect.max.y];
            let filler = Filler::new(filler_rect, background);
            children.push(Box::new(filler) as Box<dyn View>);
        }

        children
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fonts)))]
    pub fn update_selection(&mut self, selected: Category, fonts: &mut Fonts) {
        if self.selected == selected {
            return;
        }

        self.update_content(selected, fonts);
    }

    pub fn resize_by(&mut self, _delta_y: i32, _fonts: &mut Fonts) -> i32 {
        unimplemented!("there is no need for this bar to be resizable");
    }

    pub fn shift(&mut self, delta: Point) {
        self.rect += delta;
        for child in &mut self.children {
            *child.rect_mut() += delta;
        }
    }
}

impl View for CategoryNavigationBar {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
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

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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
