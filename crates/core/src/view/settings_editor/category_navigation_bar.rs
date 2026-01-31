use super::category::Category;
use super::category_button::CategoryButton;
use crate::context::Context;
use crate::font::Fonts;
use crate::framebuffer::Framebuffer;
use crate::geom::{Point, Rectangle};
use crate::view::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};

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
    pub fn new(rect: Rectangle, selected: Category) -> Self {
        let id = ID_FEEDER.next();
        let children = Self::build_category_buttons(rect, selected);

        CategoryNavigationBar {
            id,
            rect,
            children,
            selected,
        }
    }

    fn build_category_buttons(rect: Rectangle, selected: Category) -> Vec<Box<dyn View>> {
        let mut children = Vec::new();
        let categories = Category::all();
        let button_width = rect.width() as i32 / categories.len() as i32;

        for (index, category) in categories.iter().enumerate() {
            let x_min = rect.min.x + (index as i32 * button_width);
            let x_max = if index == categories.len() - 1 {
                rect.max.x
            } else {
                x_min + button_width
            };

            let button_rect = rect![x_min, rect.min.y, x_max, rect.max.y];
            let is_selected = *category == selected;

            let button = CategoryButton::new(button_rect, *category, is_selected);
            children.push(Box::new(button) as Box<dyn View>);
        }

        children
    }

    pub fn update_selection(&mut self, selected: Category) {
        if self.selected == selected {
            return;
        }

        self.selected = selected;

        for child in &mut self.children {
            if let Some(button) = child.downcast_mut::<CategoryButton>() {
                let is_selected = button.category == selected;
                button.update_selected(is_selected);
            }
        }
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
