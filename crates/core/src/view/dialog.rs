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
        let message_width = max_line_width.max(min_message_width) + 3 * padding;

        let button_count = self.buttons.len().max(1);
        let mut max_button_text_width = 0;
        for (text, _) in &self.buttons {
            let plan = font.plan(text, Some(max_button_width), None);
            max_button_text_width = max_button_text_width.max(plan.width);
        }
        let button_width = max_button_text_width + padding;

        let required_button_area_width =
            button_count as i32 * button_width + (button_count as i32 + 1) * padding;
        let dialog_width = message_width.max(required_button_area_width);
        let dialog_height = label_height + button_height + 3 * padding;

        let dx = (width as i32 - dialog_width) / 2;
        let dy = (height as i32 - dialog_height) / 2;
        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        let mut children: Vec<Box<dyn View>> = Vec::new();
        for line in &text_lines {
            let label = Label::new(Rectangle::default(), line.to_string(), Align::Center);
            children.push(Box::new(label) as Box<dyn View>);
        }
        for (text, event) in &self.buttons {
            let button = Button::new(Rectangle::default(), event.clone(), text.clone());
            children.push(Box::new(button) as Box<dyn View>);
        }

        let mut dialog = Dialog {
            id,
            rect,
            children,
            view_id: self.view_id,
            button_count,
            button_width,
        };

        dialog.layout_children(&mut context.fonts);

        dialog
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
    /// Content-based button width computed once during [`DialogBuilder::build`]
    /// from the widest button text. Reused by [`layout_children`](Dialog::layout_children)
    /// on every resize so buttons keep their text-proportional sizing.
    button_width: i32,
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

    /// Position all child views within the current dialog rect.
    ///
    /// Labels are stacked vertically with one `padding` inset from each edge.
    /// Buttons use a content-based width ([`button_width`](Dialog::button_width))
    /// and are centered horizontally with even spacing.
    ///
    /// Both [`DialogBuilder::build`] and [`Dialog::resize`] delegate to this
    /// method so the layout algorithm is defined in a single place.
    fn layout_children(&mut self, fonts: &mut Fonts) {
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;
        let line_height = font.line_height();
        let button_height = 4 * x_height;

        let label_count = self.children.len() - self.button_count;

        for i in 0..label_count {
            let y_offset = self.rect.min.y + padding + (i as i32 * line_height);
            *self.children[i].rect_mut() = rect![
                self.rect.min.x + padding,
                y_offset,
                self.rect.max.x - padding,
                y_offset + line_height
            ];
        }

        let button_area_width = self.rect.width() as i32 - 2 * padding;
        let button_spacing = (button_area_width - self.button_count as i32 * self.button_width)
            / (self.button_count as i32 + 1);

        for idx in 0..self.button_count {
            let x_offset = self.rect.min.x
                + padding
                + (idx as i32 + 1) * button_spacing
                + idx as i32 * self.button_width;
            *self.children[label_count + idx].rect_mut() = rect![
                x_offset,
                self.rect.max.y - button_height - padding,
                x_offset + self.button_width,
                self.rect.max.y - padding
            ];
        }
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
        let (width, height) = context.display.dims;
        let dialog_width = self.rect.width() as i32;
        let dialog_height = self.rect.height() as i32;

        let dx = (width as i32 - dialog_width) / 2;
        let dy = (height as i32 - dialog_height) / 2;
        self.rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        self.layout_children(&mut context.fonts);

        for child in &mut self.children {
            let rect = *child.rect();
            child.resize(rect, hub, rq, context);
        }
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

#[cfg(test)]
impl Dialog {
    fn rect_for_test(&self) -> &Rectangle {
        &self.rect
    }

    fn button_count_for_test(&self) -> usize {
        self.button_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;

    #[test]
    fn dialog_width_should_not_be_static() {
        let mut context = create_test_context();

        let dialog = Dialog::builder(ViewId::BookMenu, "Where to check for updates?".to_string())
            .add_button("Stable Release", Event::Close(ViewId::BookMenu))
            .add_button("Main Branch", Event::Close(ViewId::BookMenu))
            .add_button("PR Build", Event::Close(ViewId::BookMenu))
            .build(&mut context);

        let dialog2 = Dialog::builder(ViewId::BookMenu, "Where to check for updates?".to_string())
            .add_button("Stable Release", Event::Close(ViewId::BookMenu))
            .build(&mut context);

        let dialog1_rect = dialog.rect_for_test();
        let dialog1_width = dialog1_rect.width() as i32;
        let dialog2_rect = dialog2.rect_for_test();
        let dialog2_width = dialog2_rect.width() as i32;

        assert!(
            dialog1_width > dialog2_width,
            "Expected triple button dialog to be wider than single button: {}--{}",
            dialog1_width,
            dialog2_width
        );
    }
    #[test]
    fn dialog_width_with_three_buttons_should_expand() {
        let mut context = create_test_context();

        let dialog = Dialog::builder(ViewId::BookMenu, "Where to check for updates?".to_string())
            .add_button("Stable Release", Event::Close(ViewId::BookMenu))
            .add_button("Main Branch", Event::Close(ViewId::BookMenu))
            .add_button("PR Build", Event::Close(ViewId::BookMenu))
            .build(&mut context);

        let dialog_rect = dialog.rect_for_test();
        let dialog_width = dialog_rect.width() as i32;

        assert!(
            dialog_width > 0,
            "Dialog width should be positive, got {}",
            dialog_width
        );

        assert_eq!(
            dialog.button_count_for_test(),
            3,
            "Dialog should have 3 buttons"
        );
    }

    #[test]
    fn dialog_width_single_button_should_be_valid() {
        let mut context = create_test_context();

        let dialog = Dialog::builder(ViewId::BookMenu, "Confirm deletion?".to_string())
            .add_button("Cancel", Event::Close(ViewId::BookMenu))
            .build(&mut context);

        let dialog_rect = dialog.rect_for_test();
        let dialog_width = dialog_rect.width() as i32;

        assert!(
            dialog_width > 0,
            "Dialog width should be positive, got {}",
            dialog_width
        );

        assert_eq!(
            dialog.button_count_for_test(),
            1,
            "Dialog should have 1 button"
        );
    }

    #[test]
    fn dialog_should_center_on_display() {
        if std::env::var("TEST_ROOT_DIR").is_err() {
            return;
        }

        let mut context = create_test_context();

        let dialog = Dialog::builder(ViewId::BookMenu, "Test message".to_string())
            .add_button("OK", Event::Close(ViewId::BookMenu))
            .build(&mut context);

        let rect = dialog.rect_for_test();
        let dialog_width = rect.width();
        let dialog_height = rect.height();
        let dialog_x = rect.min.x as u32;
        let dialog_y = rect.min.y as u32;

        let expected_x = (context.display.dims.0 - dialog_width) / 2;
        let expected_y = (context.display.dims.1 - dialog_height) / 2;

        assert_eq!(
            dialog_x, expected_x,
            "Dialog X position should be centered: got {}, expected {}",
            dialog_x, expected_x
        );
        assert_eq!(
            dialog_y, expected_y,
            "Dialog Y position should be centered: got {}, expected {}",
            dialog_y, expected_y
        );
    }
}
