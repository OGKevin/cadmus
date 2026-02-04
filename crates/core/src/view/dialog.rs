//! A modal dialog view that displays a message and custom buttons.
//!
//! The dialog component provides a flexible way to display modal dialogs with a title
//! message and multiple custom buttons. Dialogs are centered on the display and render
//! with a bordered white background.
//!
//! # Building a Dialog
//!
//! Use the [`Dialog::builder`] method to create a dialog with a fluent API:
//!
//! ```no_run
//! use cadmus_core::view::dialog::Dialog;
//! use cadmus_core::view::{Event, ViewId};
//!
//! # let mut context = unsafe { std::mem::zeroed() };
//! let dialog = Dialog::builder(ViewId::BookMenu, "Confirm deletion?".to_string())
//!     .add_button("Cancel", Event::Close(ViewId::BookMenu))
//!     .add_button("Delete", Event::Close(ViewId::BookMenu))
//!     .build(&mut context);
//! ```
//!
//! # Behavior
//!
//! - **Multi-line messages**: The title supports multi-line text via newline characters
//! - **Dynamic layout**: Buttons are evenly distributed horizontally regardless of count
//! - **Button events**: When a button is tapped, it sends the event configured for that button.
//!   To close the dialog, you can either make the button event an [`Event::Close`] or handle
//!   the event in your view logic to remove the dialog from the view hierarchy.
//! - **Outside tap**: Tapping outside the dialog area automatically sends an [`Event::Close`]
//!
//! # Example: Adding to a View
//!
//! ```no_run
//! use cadmus_core::view::dialog::Dialog;
//! use cadmus_core::view::{Event, ViewId, View};
//!
//! # let mut context = unsafe { std::mem::zeroed() };
//! # let mut view_children: Vec<Box<dyn View>> = Vec::new();
//! let dialog = Dialog::builder(ViewId::BookMenu, "Save changes?".to_string())
//!     .add_button("Discard", Event::Close(ViewId::BookMenu))
//!     .add_button("Save", Event::Close(ViewId::BookMenu))
//!     .build(&mut context);
//!
//! // Add the dialog to your view hierarchy
//! view_children.push(Box::new(dialog) as Box<dyn View>);
//! ```
//!
//! [`Event`]: super::Event

use super::button::Button;
use super::label::Label;
use super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ViewId, ID_FEEDER};
use super::{BORDER_RADIUS_MEDIUM, THICKNESS_LARGE};
use crate::color::{BLACK, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::{BorderSpec, CornerSpec, Rectangle};
use crate::gesture::GestureEvent;
use crate::unit::scale_by_dpi;

/// Builder for constructing a [`Dialog`] with custom buttons and message.
///
/// Use [`Dialog::builder`] to create a new builder, then chain calls to
/// [`add_button`](DialogBuilder::add_button) to define the buttons, and finally
/// call [`build`](DialogBuilder::build) to create the dialog.
///
/// # Example
///
/// ```no_run
/// use cadmus_core::view::dialog::Dialog;
/// use cadmus_core::view::{Event, ViewId};
///
/// // Note: In actual use, `context` is provided by the application.
/// // Dialog::builder requires a properly initialized Context with
/// // Display and Fonts, so we show the API pattern here.
/// # let mut context = unsafe { std::mem::zeroed() };
/// let dialog = Dialog::builder(ViewId::AboutDialog, "Are you sure?".to_string())
///     .add_button("Cancel", Event::Close(ViewId::AboutDialog))
///     .add_button("Confirm", Event::Validate)
///     .build(&mut context);
/// ```
pub struct DialogBuilder {
    view_id: ViewId,
    title: String,
    buttons: Vec<(String, Event)>,
}

impl DialogBuilder {
    fn new(view_id: ViewId, title: String) -> Self {
        DialogBuilder {
            view_id,
            title,
            buttons: Vec::new(),
        }
    }

    /// Add a button to the dialog.
    ///
    /// Buttons are displayed from left to right in the order they are added.
    /// Each button sends a specific event when tapped.
    ///
    /// # Arguments
    ///
    /// * `text` - The label text displayed on the button
    /// * `event` - The event sent when the button is tapped
    ///
    /// # Returns
    ///
    /// Returns `self` to allow method chaining.
    pub fn add_button(mut self, text: &str, event: Event) -> Self {
        self.buttons.push((text.to_string(), event));
        self
    }

    /// Build the dialog with the configured title and buttons.
    ///
    /// Calculates the dialog layout, creates label and button views, and
    /// centers the dialog on the display.
    ///
    /// # Arguments
    ///
    /// * `context` - The rendering context, used for font metrics and display dimensions
    ///
    /// # Returns
    ///
    /// A new [`Dialog`] instance ready to be displayed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, context), fields(view_id = ?self.view_id, title = ?self.title)))]
    pub fn build(self, context: &mut Context) -> Dialog {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = context.display.dims;

        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let min_message_width = width as i32 / 2;
        let max_message_width = width as i32 - 3 * padding;
        let max_button_width = width as i32 / 5;
        let button_height = 4 * x_height;

        let text_lines: Vec<&str> = self.title.lines().collect();
        let line_count = text_lines.len().max(1);
        let line_height = font.line_height();

        let mut max_line_width = min_message_width;
        for line in &text_lines {
            let plan = font.plan(line, Some(max_message_width), None);
            max_line_width = max_line_width.max(plan.width);
        }

        let label_height = line_count as i32 * line_height;
        let dialog_width = max_line_width.max(min_message_width) + 3 * padding;
        let dialog_height = label_height + button_height + 3 * padding;

        let dx = (width as i32 - dialog_width) / 2;
        let dy = (height as i32 - dialog_height) / 2;
        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        for (i, line) in text_lines.iter().enumerate() {
            let y_offset = rect.min.y + padding + (i as i32 * line_height);
            let rect_label = rect![
                rect.min.x + padding,
                y_offset,
                rect.max.x - padding,
                y_offset + line_height
            ];

            let label = Label::new(rect_label, line.to_string(), Align::Center);
            children.push(Box::new(label) as Box<dyn View>);
        }

        let button_count = self.buttons.len().max(1);
        let mut max_button_text_width = 0;
        for (text, _) in &self.buttons {
            let plan = font.plan(text, Some(max_button_width), None);
            max_button_text_width = max_button_text_width.max(plan.width);
        }
        let button_width = max_button_text_width + padding;

        let button_area_width = rect.width() as i32 - 2 * padding;
        let button_spacing =
            (button_area_width - button_count as i32 * button_width) / (button_count as i32 + 1);

        for (idx, (text, event)) in self.buttons.iter().enumerate() {
            let x_offset = rect.min.x
                + padding
                + (idx as i32 + 1) * button_spacing
                + idx as i32 * button_width;
            let rect_button = rect![
                x_offset,
                rect.max.y - button_height - padding,
                x_offset + button_width,
                rect.max.y - padding
            ];
            let button = Button::new(rect_button, event.clone(), text.clone());
            children.push(Box::new(button) as Box<dyn View>);
        }

        Dialog {
            id,
            rect,
            children,
            view_id: self.view_id,
            button_count,
        }
    }
}

/// A modal dialog view that displays a message and allows users to select from custom buttons.
///
/// The dialog is centered on the display and renders a bordered rectangle containing:
/// - A title message (can be multi-line)
/// - Buttons evenly distributed horizontally
///
/// # Closing a Dialog
///
/// The dialog sends an [`Event::Close`] when the user taps outside the dialog area.
/// To close the dialog from a button tap, configure the button with a [`Event::Close`] event.
/// Other button events are propagated without closing the dialog. Which means you must handle the
/// closing of the dialog.
///
/// # Lifecycle
///
/// Create a dialog using the builder pattern via [`Dialog::builder`], which handles
/// automatic layout calculation based on the display dimensions and text content.
///
/// # Example
///
/// ```no_run
/// use cadmus_core::view::dialog::Dialog;
/// use cadmus_core::view::{Event, ViewId, View};
///
/// # let mut context = unsafe { std::mem::zeroed() };
/// let mut view_children: Vec<Box<dyn View>> = Vec::new();
///
/// // Note: In actual use, `context` is provided by the application.
/// // Dialog::builder requires a properly initialized Context with
/// // Display and Fonts, so we show the API pattern here.
/// let dialog = Dialog::builder(ViewId::BookMenu, "Delete this file?".to_string())
///     .add_button("No", Event::Close(ViewId::BookMenu))
///     .add_button("Yes", Event::Close(ViewId::BookMenu))
///     .build(&mut context);
///
/// view_children.push(Box::new(dialog) as Box<dyn View>);
/// ```
pub struct Dialog {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    view_id: ViewId,
    button_count: usize,
}

impl Dialog {
    /// Create a builder for a new dialog.
    ///
    /// # Arguments
    ///
    /// * `view_id` - Unique identifier for the dialog
    /// * `title` - The message text to display (supports multi-line text)
    ///
    /// # Returns
    ///
    /// A [`DialogBuilder`] that can be configured with buttons via [`add_button`](DialogBuilder::add_button).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use cadmus_core::view::dialog::Dialog;
    /// use cadmus_core::view::{Event, ViewId};
    ///
    /// # let mut context = unsafe { std::mem::zeroed() };
    /// let _dialog = Dialog::builder(ViewId::BookMenu, "Are you sure?".to_string())
    ///     .add_button("Cancel", Event::Close(ViewId::BookMenu))
    ///     .add_button("OK", Event::Validate)
    ///     .build(&mut context);
    /// ```
    pub fn builder(view_id: ViewId, title: String) -> DialogBuilder {
        DialogBuilder::new(view_id, title)
    }
}

impl View for Dialog {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, _bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(center)) if !self.rect.includes(center) => {
                hub.send(Event::Close(self.view_id)).ok();
                true
            }
            Event::Gesture(..) => true,
            _ => false,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, _fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {
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
    }

    fn resize(&mut self, _rect: Rectangle, hub: &Hub, rq: &mut RenderQueue, context: &mut Context) {
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = context.display.dims;
        let dialog_width = self.rect.width() as i32;
        let dialog_height = self.rect.height() as i32;

        let (x_height, padding) = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            let x_height = font.x_heights.0 as i32;
            let padding = font.em() as i32;
            (x_height, padding)
        };
        let button_height = 4 * x_height;

        let dx = (width as i32 - dialog_width) / 2;
        let dy = (height as i32 - dialog_height) / 2;
        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let line_height = font.line_height();

        let label_count = self.children.len() - self.button_count;

        for i in 0..label_count {
            let y_offset = rect.min.y + padding + (i as i32 * line_height);
            let label_rect = rect![
                rect.min.x + padding,
                y_offset,
                rect.max.x - padding,
                y_offset + line_height
            ];
            self.children[i].resize(label_rect, hub, rq, context);
        }

        let button_area_width = rect.width() as i32 - 2 * padding;
        let button_width = (button_area_width - (self.button_count as i32 + 1) * padding)
            / self.button_count as i32;

        for (idx, i) in (label_count..self.children.len()).enumerate() {
            let x_offset = rect.min.x + padding + (idx as i32) * (button_width + padding);
            let button_rect = rect![
                x_offset,
                rect.max.y - button_height - padding,
                x_offset + button_width,
                rect.max.y - padding
            ];
            self.children[i].resize(button_rect, hub, rq, context);
        }

        self.rect = rect;
    }

    fn is_background(&self) -> bool {
        true
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
