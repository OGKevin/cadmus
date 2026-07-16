use super::{
    Bus, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, SliderId, THICKNESS_SMALL, View,
};
use crate::color::{BLACK, PROGRESS_EMPTY, PROGRESS_FULL, PROGRESS_VALUE, WHITE};
use crate::device::{AppContext, DeviceIdentity};
use crate::font::{SLIDER_VALUE, font_from_style};
use crate::framebuffer::UpdateMode;
use crate::geom::{BorderSpec, CornerSpec, Rectangle, halves};
use crate::input::{DeviceEvent, FingerStatus};
use crate::unit::scale_by_dpi;
use crate::view::handle_event;
use crate::view::icon::Icon;

const PROGRESS_HEIGHT: f32 = 7.0;
const BUTTON_DIAMETER: f32 = 46.0;

pub struct Slider {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    slider_id: SliderId,
    value: f32,
    min_value: f32,
    max_value: f32,
    active: bool,
    last_x: i32,
}

impl Slider {
    pub fn new(
        rect: Rectangle,
        slider_id: SliderId,
        value: f32,
        min_value: f32,
        max_value: f32,
    ) -> Slider {
        Slider {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            slider_id,
            value,
            min_value,
            max_value,
            active: false,
            last_x: -1,
        }
    }

    pub fn increment(&mut self, amount: f32, rq: &mut RenderQueue) {
        self.update(self.value + amount, rq)
    }

    pub fn update_value(&mut self, x_hit: i32, dpi: u16) {
        let button_diameter = scale_by_dpi(BUTTON_DIAMETER, dpi) as i32;
        let (small_radius, big_radius) = halves(button_diameter);
        let x_offset = x_hit
            .max(self.rect.min.x + small_radius)
            .min(self.rect.max.x - big_radius);
        let progress = ((x_offset - self.rect.min.x - small_radius) as f32
            / (self.rect.width() as i32 - button_diameter) as f32)
            .clamp(0.0, 1.0);
        self.value = self.min_value + progress * (self.max_value - self.min_value);
    }

    pub fn update(&mut self, value: f32, rq: &mut RenderQueue) {
        if (self.value - value).abs() >= f32::EPSILON {
            self.value = value;
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        }
    }
}

impl View for Slider {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, bus, rq, _context), fields(event = ?evt
    ), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::Device(DeviceEvent::Finger {
                status, position, ..
            }) => match status {
                FingerStatus::Down if self.rect.includes(position) => {
                    self.active = true;
                    self.update_value(position.x, _context.device.dpi());
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    bus.push_back(Event::Slider(self.slider_id, self.value, status));
                    self.last_x = position.x;
                    true
                }
                FingerStatus::Motion if self.active && position.x != self.last_x => {
                    self.update_value(position.x, _context.device.dpi());
                    rq.add(RenderData::no_wait(
                        self.id,
                        self.rect,
                        UpdateMode::FastMono,
                    ));
                    bus.push_back(Event::Slider(self.slider_id, self.value, status));
                    self.last_x = position.x;
                    true
                }
                FingerStatus::Up if self.active => {
                    self.active = false;
                    if position.x != self.last_x {
                        self.update_value(position.x, _context.device.dpi());
                        self.last_x = position.x;
                    }
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    bus.push_back(Event::Slider(self.slider_id, self.value, status));
                    true
                }
                _ => self.active,
            },
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _rect, context), fields(rect = ?_rect
    )))]
    fn render(&self, context: &mut AppContext, _rect: Rectangle) {
        let (fb, fonts, dpi) = context.framebuffer_and_fonts();
        let progress_height = scale_by_dpi(PROGRESS_HEIGHT, dpi) as i32;
        let button_diameter = scale_by_dpi(BUTTON_DIAMETER, dpi) as i32;
        let border_thickness = scale_by_dpi(THICKNESS_SMALL, dpi) as u16;

        let progress = (self.value - self.min_value) / (self.max_value - self.min_value);
        let (small_radius, big_radius) = halves(button_diameter);
        let x_offset = self.rect.min.x
            + small_radius
            + ((self.rect.width() as f32 - button_diameter as f32) * progress) as i32;

        fb.draw_rectangle(&self.rect, WHITE);

        let (small_mini_radius, big_mini_radius) = halves(progress_height);
        let (small_padding, big_padding) = halves(self.rect.height() as i32 - progress_height);
        let rect = rect![
            self.rect.min.x + small_radius - big_mini_radius,
            self.rect.min.y + small_padding,
            self.rect.max.x - big_radius + small_mini_radius,
            self.rect.max.y - big_padding
        ];

        fb.draw_rounded_rectangle_with_border(
            &rect,
            &CornerSpec::Uniform(small_mini_radius),
            &BorderSpec {
                thickness: border_thickness,
                color: BLACK,
            },
            &|x, _| {
                if x < x_offset {
                    PROGRESS_FULL
                } else {
                    PROGRESS_EMPTY
                }
            },
        );

        let (small_padding, big_padding) = halves(self.rect.height() as i32 - button_diameter);
        let rect = rect![
            x_offset - small_radius,
            self.rect.min.y + small_padding,
            x_offset + big_radius,
            self.rect.max.y - big_padding
        ];
        let fill_color = if self.active { BLACK } else { WHITE };

        fb.draw_rounded_rectangle_with_border(
            &rect,
            &CornerSpec::Uniform(small_radius),
            &BorderSpec {
                thickness: 2 * border_thickness,
                color: BLACK,
            },
            &fill_color,
        );

        let font = font_from_style(fonts, &SLIDER_VALUE, dpi);
        let plan = font.plan(&format!("{:.1}", self.value), None, None);
        let x_height = font.x_heights.1 as i32;

        let x_drift = if self.value > (self.min_value + self.max_value) / 2.0 {
            -(small_radius + plan.width)
        } else {
            small_radius
        };

        let pt = pt!(
            x_offset + x_drift,
            self.rect.min.y + x_height.max(small_padding)
        );
        font.render(fb, PROGRESS_VALUE, &plan, pt);
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

pub struct SliderWithButtons {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl SliderWithButtons {
    pub fn new(
        rect: Rectangle,
        slider_id: SliderId,
        value: f32,
        min_value: f32,
        max_value: f32,
    ) -> SliderWithButtons {
        let decrement = Icon::new(
            "minus",
            rect![rect.min.x, rect.min.y, rect.min.x + 40, rect.max.y],
            Event::SliderIncrement(-1 as f32),
        );
        let slider = Slider::new(
            rect![
                decrement.rect.max.x,
                rect.min.y,
                rect.max.x - 40,
                rect.max.y
            ],
            slider_id,
            value,
            min_value,
            max_value,
        );
        let increment = Icon::new(
            "plus",
            rect![slider.rect.max.x, rect.min.y, rect.max.x, rect.max.y],
            Event::SliderIncrement(1 as f32),
        );
        SliderWithButtons {
            rect: rect,
            id: ID_FEEDER.next(),
            children: vec![Box::new(decrement), Box::new(slider), Box::new(increment)],
        }
    }
}

impl View for SliderWithButtons {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, bus, rq, _context), fields(event = ?evt
    ), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::SliderIncrement(amount) => {
                let id = self.id;
                let rect = self.rect;
                let slider = self.children_mut()[1].downcast_mut::<Slider>().unwrap();
                slider.increment(amount, rq);
                rq.add(RenderData::new(id, rect, UpdateMode::Gui));
                bus.push_back(Event::Slider(
                    slider.slider_id,
                    slider.value,
                    FingerStatus::Up,
                ));
                true
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _rect, _context), fields(rect = ?_rect)))]
    fn render(&self, _context: &mut AppContext, _rect: Rectangle) {}

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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use crate::gesture::GestureEvent;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    #[test]
    fn test_tap_decrements_value_and_emits_event() {
        let bounds = rect![0, 0, 200, 50];
        let mut slider = SliderWithButtons::new(bounds, SliderId::LightIntensity, 7.0, 5.0, 6.0);

        let (hub, _receiver) = channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context();

        let dec_bounds = slider.child(0).rect();
        let point = Point::new(
            dec_bounds.min.x + dec_bounds.max.x / 2,
            dec_bounds.min.y + dec_bounds.max.y / 2,
        );
        let tap_event = Event::Gesture(GestureEvent::Tap(point));
        crate::view::handle_event(
            slider.child_mut(0),
            &tap_event,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        assert_eq!(bus.len(), 1);
        let increment_event = bus.pop_front().unwrap();

        crate::view::handle_event(
            &mut slider,
            &increment_event,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );
        assert_eq!(bus.len(), 1);
        let update_event = bus.pop_front();
        assert!(matches!(
            update_event,
            Some(Event::Slider(
                SliderId::LightIntensity,
                6.0,
                FingerStatus::Up
            ))
        ));
    }
}
