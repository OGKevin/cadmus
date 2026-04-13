use super::super::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::color::{TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{Point, Rectangle};
use chrono::{Datelike, Local, TimeZone, Timelike};
use tracing::info;

/// A leaf view that renders a full-screen calendar for the current month.
///
/// Displays the month title, current time, an optional "auto power off in N
/// min" countdown, weekday headers, and a day grid with today highlighted.
pub(super) struct CalendarView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    minutes_until_poweroff: Option<i64>,
    /// When true the background is inverted (halt screen colour scheme).
    halt: bool,
}

impl CalendarView {
    pub(super) fn new(rect: Rectangle, minutes_until_poweroff: Option<i64>, halt: bool) -> Self {
        CalendarView {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            minutes_until_poweroff,
            halt,
        }
    }
}

impl View for CalendarView {
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

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let scheme = if self.halt {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };

        fb.draw_rectangle(&self.rect, scheme[0]);

        let now = Local::now();
        info!(timestamp = %now, "Rendering calendar view");

        let year = now.year();
        let month = now.month();
        let today = now.day() as i32;

        let month_names = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        let title = format!("{} {}", month_names[(month - 1) as usize], year);
        let time_str = format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second());

        let days_in_month = {
            let (next_year, next_month) = if month == 12 {
                (year + 1, 1)
            } else {
                (year, month + 1)
            };
            let next_month_start = Local
                .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
                .unwrap();
            (next_month_start - chrono::Duration::days(1)).day() as i32
        };

        let first_of_month = Local.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap();
        let starting_weekday = first_of_month.weekday().num_days_from_sunday() as i32;

        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let line_height = x_height * 2;

        let title_plan = font.plan(&title, None, None);
        let title_dx = (self.rect.width() as i32 - title_plan.width) / 2;
        let title_dy = x_height * 2;
        font.render(fb, scheme[1], &title_plan, Point::new(title_dx, title_dy));

        let time_plan = font.plan(&time_str, None, None);
        let time_dx = (self.rect.width() as i32 - time_plan.width) / 2;
        let time_dy = title_dy + line_height;
        font.render(fb, scheme[1], &time_plan, Point::new(time_dx, time_dy));

        let grid_offset_y = if let Some(minutes) = self.minutes_until_poweroff {
            let poweroff_str = format!("Auto power off in {} min", minutes);
            let poweroff_plan = font.plan(&poweroff_str, None, None);
            let poweroff_dx = (self.rect.width() as i32 - poweroff_plan.width) / 2;
            let poweroff_dy = time_dy + line_height;
            font.render(
                fb,
                scheme[1],
                &poweroff_plan,
                Point::new(poweroff_dx, poweroff_dy),
            );
            poweroff_dy
        } else {
            time_dy
        };

        let weekdays = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
        let cell_width = self.rect.width() as i32 / 7;
        let cell_height = line_height;

        let grid_start_y = grid_offset_y + line_height + x_height;

        for (i, day_name) in weekdays.iter().enumerate() {
            let plan = font.plan(day_name, None, None);
            let dx = (cell_width - plan.width) / 2 + (i as i32 * cell_width);
            font.render(fb, scheme[1], &plan, Point::new(dx, grid_start_y));
        }

        let days_start_y = grid_start_y + cell_height + x_height;
        let mut day_num = 1i32;

        'outer: for week in 0..6 {
            for weekday in 0..7 {
                if week == 0 && weekday < starting_weekday {
                    continue;
                }

                if day_num > days_in_month {
                    break 'outer;
                }

                let cell_x = weekday * cell_width;
                let cell_y = days_start_y + week * cell_height;
                let day_str = day_num.to_string();
                let plan = font.plan(&day_str, None, None);
                let dx = (cell_width - plan.width) / 2 + cell_x;

                if day_num == today {
                    let box_padding = x_height / 2;
                    let highlight = Rectangle::new(
                        pt!(
                            cell_x + (cell_width - plan.width) / 2 - box_padding,
                            cell_y - box_padding / 2
                        ),
                        pt!(
                            cell_x + (cell_width + plan.width) / 2 + box_padding,
                            cell_y + x_height + box_padding / 2
                        ),
                    );
                    fb.draw_rectangle(&highlight, scheme[1]);
                    font.render(fb, scheme[0], &plan, Point::new(dx, cell_y));
                } else {
                    font.render(fb, scheme[1], &plan, Point::new(dx, cell_y));
                }

                day_num += 1;
            }
        }
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
