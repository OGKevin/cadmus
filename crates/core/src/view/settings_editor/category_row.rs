use super::super::label::Label;
use super::super::Align;
use super::super::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ID_FEEDER};
use super::category::Category;
use crate::color::{TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::input::{DeviceEvent, FingerStatus};
use crate::view::button::Button;

pub struct CategoryRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    category: Category,
}

impl CategoryRow {
    pub fn new(category: Category, rect: Rectangle, _context: &Context) -> CategoryRow {
        let mut children = Vec::new();

        let button = Button::new(
            rect,
            Event::OpenSettingsCategory(category),
            category.label(),
        );
        children.push(Box::new(button) as Box<dyn View>);

        CategoryRow {
            id: ID_FEEDER.next(),
            rect,
            children,
            category,
        }
    }
}

impl View for CategoryRow {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(point)) if self.rect.includes(point) => {
                bus.push_back(Event::OpenSettingsCategory(self.category));
                true
            }
            _ => false,
        }
    }

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
}
