//! Notification view component for displaying temporary or persistent messages.
//!
//! # Examples
//!
//! ## Auto-dismissing notification
//!
//! ```
//! use cadmus_core::view::notification::Notification;
//! use cadmus_core::view::{Event, NotificationEvent};
//!
//! let (tx, rx) = std::sync::mpsc::channel();
//! // Send via event for standard notifications
//! tx.send(Event::Notification(NotificationEvent::Show("File saved successfully.".to_string()))).ok();
//! ```
//!
//! ## Pinned notification with progress bar
//!
//! ```
//! use cadmus_core::view::{Event, NotificationEvent, ViewId, ID_FEEDER};
//! let (tx, rx) = std::sync::mpsc::channel();
//! // Create a pinned notification with a custom ID
//! let download_id = ViewId::MessageNotif(ID_FEEDER.next());
//! tx.send(Event::Notification(NotificationEvent::ShowPinned(download_id, "Download: 0%".to_string()))).ok();
//!
//! // Update the notification text as progress changes
//! tx.send(Event::Notification(NotificationEvent::UpdateText(
//!     download_id,
//!     "Download: 50%".to_string()
//! ))).ok();
//!
//! // Update the progress bar (0-100)
//! tx.send(Event::Notification(NotificationEvent::UpdateProgress(download_id, 50))).ok();
//!
//! // Dismiss when done
//! tx.send(Event::Close(download_id)).ok();
//! ```

use super::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use super::{BORDER_RADIUS_MEDIUM, SMALL_BAR_HEIGHT, THICKNESS_LARGE};
use crate::color::{BLACK, TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{BorderSpec, CornerSpec, Rectangle};
use crate::gesture::GestureEvent;
use crate::input::DeviceEvent;
use crate::unit::scale_by_dpi;
use std::thread;
use std::time::Duration;

const NOTIFICATION_CLOSE_DELAY: Duration = Duration::from_secs(4);

/// Events related to notifications.
#[derive(Debug, Clone)]
pub enum NotificationEvent {
    /// Show a standard auto-dismissing notification.
    Show(String),
    /// Show a pinned notification that persists until dismissed.
    ShowPinned(ViewId, String),
    /// Update the text of a pinned notification.
    UpdateText(ViewId, String),
    /// Update the progress of a pinned notification (0-100).
    UpdateProgress(ViewId, u8),
}

/// A notification view that displays temporary or persistent messages.
///
/// Notifications can either auto-dismiss after 4 seconds (standard notifications)
/// or persist until manually dismissed (pinned notifications). Pinned notifications
/// can also display an optional progress bar for long-running operations.
///
/// Notifications are positioned in a 3x2 grid at the top of the screen, alternating
/// between left and right sides to avoid overlapping.
pub struct Notification {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    text: String,
    max_width: i32,
    index: u8,
    view_id: ViewId,
    progress: Option<u8>,
}

impl Notification {
    /// Creates a new notification.
    ///
    /// # Arguments
    ///
    /// * `view_id` - Optional ViewId for the notification. If None, generates a new one.
    /// * `text` - The message to display
    /// * `pinned` - If `false`, notification auto-dismisses after 4 seconds. If `true`, persists until dismissed.
    /// * `hub` - Event hub for sending close events
    /// * `rq` - Render queue for scheduling display updates
    /// * `context` - Application context containing fonts, display dimensions, and notification index
    ///
    /// # Returns
    ///
    /// A new `Notification` instance with `progress` initialized to `None`.
    pub fn new(
        view_id: Option<ViewId>,
        text: String,
        pinned: bool,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> Notification {
        let id = ID_FEEDER.next();
        let view_id = view_id.unwrap_or(ViewId::MessageNotif(id));
        let index = context.notification_index;

        if !pinned {
            let hub2 = hub.clone();
            thread::spawn(move || {
                thread::sleep(NOTIFICATION_CLOSE_DELAY);
                hub2.send(Event::Close(view_id)).ok();
            });
        }

        let dpi = CURRENT_DEVICE.dpi;
        let (width, _) = context.display.dims;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;

        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let max_message_width = width as i32 - 5 * padding;
        let plan = font.plan(&text, Some(max_message_width), None);

        let dialog_width = plan.width + 3 * padding;
        let dialog_height = 7 * x_height;

        let side = (index / 3) % 2;
        let dx = if side == 0 {
            width as i32 - dialog_width - padding
        } else {
            padding
        };
        let dy = small_height + padding + (index % 3) as i32 * (dialog_height + padding);

        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        rq.add(RenderData::new(id, rect, UpdateMode::Gui));
        context.notification_index = index.wrapping_add(1);

        Notification {
            id,
            rect,
            children: Vec::new(),
            text,
            max_width: max_message_width,
            index,
            view_id,
            progress: None,
        }
    }

    /// Updates the text content of the notification and schedules a re-render.
    ///
    /// # Arguments
    ///
    /// * `text` - The new message text to display
    /// * `rq` - Render queue for scheduling the display update
    ///
    /// # Note
    ///
    /// This method does not recalculate the notification's position or size.
    /// The text will be re-wrapped within the existing notification bounds.
    pub fn update_text(&mut self, text: String, rq: &mut RenderQueue) {
        self.text = text;
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    /// Updates the progress percentage of the notification and schedules a re-render.
    ///
    /// # Arguments
    ///
    /// * `progress` - Progress percentage (0-100). Values outside this range will be clamped during rendering.
    /// * `rq` - Render queue for scheduling the display update
    ///
    /// # Note
    ///
    /// The progress bar is displayed as a thin horizontal line below the text.
    /// Setting progress to `None` via direct field access will hide the progress bar.
    pub fn update_progress(&mut self, progress: u8, rq: &mut RenderQueue) {
        self.progress = Some(progress);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }
}

impl View for Notification {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(center) => true,
            Event::Gesture(GestureEvent::Swipe { start, .. }) if self.rect.includes(start) => true,
            Event::Device(DeviceEvent::Finger { position, .. }) if self.rect.includes(position) => {
                true
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;

        let border_radius = scale_by_dpi(BORDER_RADIUS_MEDIUM, dpi) as i32;
        let border_thickness = scale_by_dpi(THICKNESS_LARGE, dpi) as u16;

        fb.draw_rounded_rectangle_with_border(
            &self.rect,
            &CornerSpec::Uniform(border_radius),
            &BorderSpec {
                thickness: border_thickness,
                color: BLACK,
            },
            &WHITE,
        );

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let plan = font.plan(&self.text, Some(self.max_width), None);
        let x_height = font.x_heights.0 as i32;

        let dx = (self.rect.width() as i32 - plan.width) as i32 / 2;
        let dy = (self.rect.height() as i32 - x_height) / 2;
        let pt = pt!(self.rect.min.x + dx, self.rect.max.y - dy);

        font.render(fb, TEXT_NORMAL[1], &plan, pt);

        if let Some(progress) = self.progress {
            let progress_clamped = progress.min(100);
            let padding = font.em() as i32;
            let progress_bar_height = scale_by_dpi(2.0, dpi) as i32;
            let progress_bar_width = self.rect.width() as i32 - 2 * padding;
            let progress_bar_y = self.rect.max.y - padding - progress_bar_height;

            let progress_bg_rect = rect![
                self.rect.min.x + padding,
                progress_bar_y,
                self.rect.min.x + padding + progress_bar_width,
                progress_bar_y + progress_bar_height
            ];
            fb.draw_rectangle(&progress_bg_rect, TEXT_NORMAL[0]);

            let filled_width = (progress_bar_width * progress_clamped as i32) / 100;
            if filled_width > 0 {
                let progress_fill_rect = rect![
                    self.rect.min.x + padding,
                    progress_bar_y,
                    self.rect.min.x + padding + filled_width,
                    progress_bar_y + progress_bar_height
                ];
                fb.draw_rectangle(&progress_fill_rect, BLACK);
            }
        }
    }

    fn resize(
        &mut self,
        _rect: Rectangle,
        _hub: &Hub,
        _rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = context.display.dims;
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let side = (self.index / 3) % 2;
        let padding = if side == 0 {
            height as i32 - self.rect.max.x
        } else {
            self.rect.min.x
        };
        let dialog_width = self.rect.width() as i32;
        let dialog_height = self.rect.height() as i32;
        let dx = if side == 0 {
            width as i32 - dialog_width - padding
        } else {
            padding
        };
        let dy = small_height + padding + (self.index % 3) as i32 * (dialog_height + padding);
        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];
        self.rect = rect;
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

    fn view_id(&self) -> Option<ViewId> {
        Some(self.view_id)
    }
}
