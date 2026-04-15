use super::super::{Bus, Event, Hub, Id, RenderQueue, View, ID_FEEDER};
use crate::color::{Color, TEXT_INVERTED_HARD, TEXT_NORMAL};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::font::{font_from_style, Fonts, DISPLAY_FONT_SIZE, FONT_SIZES, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{CornerSpec, Point, Rectangle};
use chrono::{Datelike, Local, NaiveDate, Timelike};
use std::time::Duration;
use tracing::debug;

/// Returns the translated full month name for `month` (1-indexed).
fn month_name(month: u32) -> String {
    match month {
        1 => fl!("calendar-month-january"),
        2 => fl!("calendar-month-february"),
        3 => fl!("calendar-month-march"),
        4 => fl!("calendar-month-april"),
        5 => fl!("calendar-month-may"),
        6 => fl!("calendar-month-june"),
        7 => fl!("calendar-month-july"),
        8 => fl!("calendar-month-august"),
        9 => fl!("calendar-month-september"),
        10 => fl!("calendar-month-october"),
        11 => fl!("calendar-month-november"),
        12 => fl!("calendar-month-december"),
        _ => String::new(),
    }
}

/// Returns the translated short month name for `month` (1-indexed).
fn short_month_name(month: u32) -> String {
    match month {
        1 => fl!("calendar-month-short-jan"),
        2 => fl!("calendar-month-short-feb"),
        3 => fl!("calendar-month-short-mar"),
        4 => fl!("calendar-month-short-apr"),
        5 => fl!("calendar-month-short-may"),
        6 => fl!("calendar-month-short-jun"),
        7 => fl!("calendar-month-short-jul"),
        8 => fl!("calendar-month-short-aug"),
        9 => fl!("calendar-month-short-sep"),
        10 => fl!("calendar-month-short-oct"),
        11 => fl!("calendar-month-short-nov"),
        12 => fl!("calendar-month-short-dec"),
        _ => String::new(),
    }
}

/// Returns the translated weekday abbreviation for `weekday` (Mon=0 … Sun=6).
fn weekday_name(weekday: usize) -> String {
    match weekday {
        0 => fl!("calendar-weekday-mon"),
        1 => fl!("calendar-weekday-tue"),
        2 => fl!("calendar-weekday-wed"),
        3 => fl!("calendar-weekday-thu"),
        4 => fl!("calendar-weekday-fri"),
        5 => fl!("calendar-weekday-sat"),
        6 => fl!("calendar-weekday-sun"),
        _ => String::new(),
    }
}

/// A leaf view that renders a full-screen calendar for the current month.
///
/// Displays the current time, date, month title, weekday headers, and a day
/// grid with today highlighted. An optional power-off countdown is shown
/// between the date line and the calendar grid.
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

/// Returns the number of days in `month` of `year`.
fn days_in_month(year: i32, month: u32) -> i32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day() as i32)
        .unwrap_or(30)
}

/// Returns the Monday-based weekday index (Mon=0 … Sun=6) of the first day of
/// `month` in `year`.
fn month_start_weekday(year: i32, month: u32) -> i32 {
    NaiveDate::from_ymd_opt(year, month, 1)
        .map(|d| d.weekday().num_days_from_monday() as i32)
        .unwrap_or(0)
}

/// Returns the number of days in the month preceding `month`/`year`.
fn days_in_prev_month(year: i32, month: u32) -> i32 {
    let (prev_year, prev_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    days_in_month(prev_year, prev_month)
}

/// Returns the color used on color-capable devices, falling back to
/// `fallback` on grayscale hardware.
fn color_or(is_color: bool, r: u8, g: u8, b: u8, fallback: Color) -> Color {
    if is_color {
        Color::Rgb(r, g, b)
    } else {
        fallback
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
        debug!(timestamp = %now, "Rendering calendar view");

        let year = now.year();
        let month = now.month();
        let today = now.day() as i32;

        let is_color = CURRENT_DEVICE.color_samples() > 1;
        let month_color = color_or(is_color, 180, 60, 40, scheme[1]);
        let today_bg = color_or(is_color, 70, 100, 150, scheme[1]);

        let dpi = CURRENT_DEVICE.dpi;
        let screen_width = self.rect.width() as i32;
        let screen_height = self.rect.height() as i32;

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        font.set_size(DISPLAY_FONT_SIZE * 2, dpi);
        let time_str = format!("{:02}:{:02}", now.hour(), now.minute());
        let time_plan = font.plan(&time_str, None, None);
        let time_x = (screen_width - time_plan.width) / 2;
        let time_cap_height = font.x_heights.1 as i32;
        let time_y = screen_height * 20 / 100;
        font.render(fb, scheme[1], &time_plan, Point::new(time_x, time_y));

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let normal_line_height = font.x_heights.0 as i32 * 2;
        let short_month = short_month_name(month);
        let weekday = weekday_name(now.weekday().num_days_from_monday() as usize);
        let day_str = format!("{:02}", today);
        let year_str = year.to_string();
        let date_str = fl!(
            "calendar-date-line",
            day = day_str.as_str(),
            month = short_month.as_str(),
            year = year_str.as_str(),
            weekday = weekday.as_str()
        );
        let date_plan = font.plan(&date_str, None, None);
        let date_x = (screen_width - date_plan.width) / 2;
        let date_y = time_y + time_cap_height + normal_line_height;
        font.render(fb, scheme[1], &date_plan, Point::new(date_x, date_y));

        let after_header_y = if let Some(minutes) = self.minutes_until_poweroff.filter(|&m| m > 0) {
            let duration = Duration::from_secs((minutes * 60).max(0) as u64);
            let duration_str = humantime::format_duration(duration).to_string();
            let poweroff_str = fl!("calendar-poweroff", duration = duration_str.as_str());
            let poweroff_plan = font.plan(&poweroff_str, None, None);
            let poweroff_x = (screen_width - poweroff_plan.width) / 2;
            let poweroff_y = date_y + normal_line_height;
            font.render(
                fb,
                scheme[1],
                &poweroff_plan,
                Point::new(poweroff_x, poweroff_y),
            );
            poweroff_y + normal_line_height
        } else {
            date_y + normal_line_height
        };

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        font.set_size(FONT_SIZES[1] * 2, dpi);
        let full_month = month_name(month);
        let month_plan = font.plan(&full_month, None, None);
        let month_x = (screen_width - month_plan.width) / 2;
        // 62% keeps the large clock in the upper portion and the calendar in
        // the lower portion, but we never let them overlap.
        let calendar_top = (screen_height * 62 / 100).max(after_header_y + normal_line_height);
        let month_cap_height = font.x_heights.1 as i32;
        font.render(
            fb,
            month_color,
            &month_plan,
            Point::new(month_x, calendar_top),
        );

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let separator_y = calendar_top + month_cap_height + normal_line_height / 2;
        fb.draw_segment(
            pt!(self.rect.min.x, separator_y),
            pt!(self.rect.max.x, separator_y),
            0.5,
            0.5,
            scheme[1],
        );

        let cell_width = screen_width / 7;
        let header_y = separator_y + normal_line_height;
        for i in 0..7usize {
            let name = weekday_name(i);
            let plan = font.plan(&name, None, None);
            let dx = (cell_width - plan.width) / 2 + (i as i32 * cell_width);
            font.render(fb, scheme[1], &plan, Point::new(dx, header_y));
        }

        let x_height = font.x_heights.0 as i32;
        let cell_height = normal_line_height + x_height / 2;
        let days_start_y = header_y + cell_height;

        let starting_weekday = month_start_weekday(year, month);
        let current_month_days = days_in_month(year, month);
        let prev_month_days = days_in_prev_month(year, month);

        for slot in 0..(6 * 7) {
            let week = slot / 7;
            let weekday = slot % 7;
            let cell_x = weekday * cell_width;
            let cell_y = days_start_y + week * cell_height;

            let (day_num, is_current_month) = if slot < starting_weekday {
                (prev_month_days - starting_weekday + slot + 1, false)
            } else {
                let d = slot - starting_weekday + 1;
                if d <= current_month_days {
                    (d, true)
                } else {
                    (d - current_month_days, false)
                }
            };

            let day_str = format!("{:02}", day_num);
            let plan = font.plan(&day_str, None, None);
            let dx = (cell_width - plan.width) / 2 + cell_x;

            if is_current_month && day_num == today {
                let pad = x_height / 3;
                // cell_y is the typographic baseline; digits extend upward by
                // x_height. The highlight covers from above the cap top down
                // to just below the baseline, then the text renders on top.
                let highlight = Rectangle::new(
                    pt!(cell_x + pad, cell_y - x_height - pad),
                    pt!(cell_x + cell_width - pad, cell_y + pad),
                );
                fb.draw_rounded_rectangle(&highlight, &CornerSpec::Uniform(4), today_bg);
                font.render(fb, scheme[0], &plan, Point::new(dx, cell_y));
            } else {
                let color = if is_current_month {
                    scheme[1]
                } else {
                    scheme[2]
                };
                font.render(fb, color, &plan, Point::new(dx, cell_y));
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
