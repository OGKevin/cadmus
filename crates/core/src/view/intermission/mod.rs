mod calendar;

use super::{Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View};
use crate::color::{BLACK, Color, TEXT_INVERTED_HARD, TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::document::{Location, open};
use crate::fl;
use crate::font::{DISPLAY_STYLE, Fonts, font_from_style};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::i18n::I18nDisplay;
use crate::rtc::AlarmType;
use crate::settings::{IntermKind, IntermissionDisplay};
use calendar::CalendarView;
use std::path::PathBuf;
use tracing::warn;

pub struct Intermission {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    message: Message,
    halt: bool,
}

enum Message {
    Text(String),
    Image(PathBuf),
    Cover(PathBuf),
    /// Calendar rendering is delegated entirely to the CalendarView child.
    Calendar,
    Fill(Color),
}

impl Intermission {
    pub fn new(rect: Rectangle, kind: IntermKind, context: &Context) -> Intermission {
        let halt = kind == IntermKind::PowerOff;

        let (message, children): (Message, Vec<Box<dyn View>>) =
            match &context.settings.intermissions[kind] {
                IntermissionDisplay::Logo => (Message::Text(kind.text().to_string()), Vec::new()),
                IntermissionDisplay::Cover => {
                    let msg =
                        if let Some(info) = context.library.most_recently_opened_reading_book() {
                            Message::Cover(context.library.home.join(&info.file.path))
                        } else {
                            Message::Text(kind.text().to_string())
                        };
                    (msg, Vec::new())
                }
                IntermissionDisplay::Blank => (Message::Fill(WHITE), Vec::new()),
                IntermissionDisplay::BlankInverted => (Message::Fill(BLACK), Vec::new()),
                IntermissionDisplay::Image(path) => (Message::Image(path.clone()), Vec::new()),
                IntermissionDisplay::Calendar => {
                    let minutes_until_poweroff = context
                        .alarm_manager
                        .as_ref()
                        .and_then(|am| am.time_until_alarm(AlarmType::AutoPowerOff))
                        .map(|secs| secs / 60);
                    let child = CalendarView::new(rect, minutes_until_poweroff, halt);
                    (Message::Calendar, vec![Box::new(child) as Box<dyn View>])
                }
            };

        Intermission {
            id: ID_FEEDER.next(),
            rect,
            children,
            message,
            halt,
        }
    }
}

impl I18nDisplay for IntermissionDisplay {
    fn to_i18n_string(&self) -> String {
        match self {
            IntermissionDisplay::Logo => fl!("settings-intermission-logo"),
            IntermissionDisplay::Blank => fl!("settings-intermission-blank"),
            IntermissionDisplay::BlankInverted => fl!("settings-intermission-blank-inverted"),
            IntermissionDisplay::Cover => fl!("settings-intermission-cover"),
            IntermissionDisplay::Calendar => fl!("settings-intermission-calendar"),
            IntermissionDisplay::Image(_) => fl!("settings-intermission-custom"),
        }
    }
}

impl View for Intermission {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _evt, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        true
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, fb, fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let scheme = if self.halt {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };

        fb.draw_rectangle(&self.rect, scheme[0]);

        match self.message {
            Message::Text(ref text) => {
                let dpi = CURRENT_DEVICE.dpi;

                let font = font_from_style(fonts, &DISPLAY_STYLE, dpi);
                let padding = font.em() as i32;
                let max_width = self.rect.width() as i32 - 3 * padding;
                let mut plan = font.plan(text, None, None);

                if plan.width > max_width {
                    let scale = max_width as f32 / plan.width as f32;
                    let size = (scale * DISPLAY_STYLE.size as f32) as u32;
                    font.set_size(size, dpi);
                    plan = font.plan(text, None, None);
                }

                let x_height = font.x_heights.0 as i32;

                let dx = (self.rect.width() as i32 - plan.width) / 2;
                let dy = (self.rect.height() as i32) / 3;

                font.render(fb, scheme[1], &plan, pt!(dx, dy));

                match open("icons/dodecahedron.svg") {
                    None => warn!("failed to open icons/dodecahedron.svg"),
                    Some(mut doc) => match doc.dims(0) {
                        None => warn!("failed to read dimensions from dodecahedron.svg"),
                        Some((width, height)) => {
                            let scale = (plan.width as f32 / width.max(height)) / 4.0;
                            match doc.pixmap(Location::Exact(0), scale, 1) {
                                None => warn!("failed to render pixmap from dodecahedron.svg"),
                                Some((pixmap, _)) => {
                                    let dx = (self.rect.width() as i32 - pixmap.width as i32) / 2;
                                    let dy = dy + 2 * x_height;
                                    let pt = self.rect.min + pt!(dx, dy);
                                    fb.draw_blended_pixmap(&pixmap, pt, scheme[1]);
                                }
                            }
                        }
                    },
                }
            }
            Message::Image(ref path) => {
                if let Some(mut doc) = open(path) {
                    if let Some((width, height)) = doc.dims(0) {
                        let w_ratio = self.rect.width() as f32 / width;
                        let h_ratio = self.rect.height() as f32 / height;
                        let scale = w_ratio.min(h_ratio);
                        if let Some((pixmap, _)) =
                            doc.pixmap(Location::Exact(0), scale, CURRENT_DEVICE.color_samples())
                        {
                            let dx = (self.rect.width() as i32 - pixmap.width as i32) / 2;
                            let dy = (self.rect.height() as i32 - pixmap.height as i32) / 2;
                            let pt = self.rect.min + pt!(dx, dy);
                            fb.draw_pixmap(&pixmap, pt);
                            if fb.inverted() {
                                let rect = pixmap.rect() + pt;
                                fb.invert_region(&rect);
                            }
                        }
                    }
                }
            }
            Message::Cover(ref path) => {
                if let Some(mut doc) = open(path) {
                    if let Some(pixmap) = doc.preview_pixmap(
                        self.rect.width() as f32,
                        self.rect.height() as f32,
                        CURRENT_DEVICE.color_samples(),
                    ) {
                        let dx = (self.rect.width() as i32 - pixmap.width as i32) / 2;
                        let dy = (self.rect.height() as i32 - pixmap.height as i32) / 2;
                        let pt = self.rect.min + pt!(dx, dy);
                        fb.draw_pixmap(&pixmap, pt);
                        if fb.inverted() {
                            let rect = pixmap.rect() + pt;
                            fb.invert_region(&rect);
                        }
                    }
                }
            }
            Message::Fill(color) => {
                fb.draw_rectangle(&self.rect, color);
            }
            // CalendarView child handles its own rendering; this arm is never
            // reached because Intermission has children in the Calendar case.
            Message::Calendar => {}
        }
    }

    fn might_rotate(&self) -> bool {
        false
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
