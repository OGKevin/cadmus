use super::super::label::Label;
use super::super::Align;
use super::super::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use super::setting_value::{Kind as ValueKind, SettingValue};
use crate::context::Context;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;

pub enum Kind {
    KeyboardLayout,
    SleepCover,
    AutoShare,
    ButtonScheme,
}

impl Kind {
    pub fn label(&self) -> String {
        match self {
            Kind::KeyboardLayout => "Keyboard Layout".to_string(),
            Kind::SleepCover => "Enable Sleep Cover".to_string(),
            Kind::AutoShare => "Enable Auto Share".to_string(),
            Kind::ButtonScheme => "Button Scheme".to_string(),
        }
    }

    fn value_kind(&self) -> ValueKind {
        match self {
            Kind::KeyboardLayout => ValueKind::KeyboardLayout,
            Kind::SleepCover => ValueKind::SleepCover,
            Kind::AutoShare => ValueKind::AutoShare,
            Kind::ButtonScheme => ValueKind::ButtonScheme,
        }
    }
}

pub struct SettingRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl SettingRow {
    pub fn new(kind: Kind, rect: Rectangle, context: &Context) -> SettingRow {
        let mut children = Vec::new();

        let half_width = rect.width() as i32 / 2;
        let label_rect = rect![rect.min.x, rect.min.y, rect.min.x + half_width, rect.max.y];
        let value_rect = rect![rect.min.x + half_width, rect.min.y, rect.max.x, rect.max.y];

        let label_text = kind.label();
        let label = Label::new(label_rect, label_text, Align::Left(50));
        children.push(Box::new(label) as Box<dyn View>);

        let setting_value = SettingValue::new(kind.value_kind(), value_rect, context);
        children.push(Box::new(setting_value) as Box<dyn View>);

        SettingRow {
            id: ID_FEEDER.next(),
            rect,
            children,
        }
    }
}

impl View for SettingRow {
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
