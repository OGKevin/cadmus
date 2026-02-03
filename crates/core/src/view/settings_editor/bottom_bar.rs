use crate::color::WHITE;
use crate::font::Fonts;
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::{Event, Id, View, ID_FEEDER};

/// Defines the layout variant for the settings editor bottom bar
#[derive(Debug, Clone)]
pub enum BottomBarVariant {
    /// Single button centered in the bar (typically for save/validate)
    SingleButton {
        /// The event to emit when the button is clicked
        event: Event,
        /// Icon name for the button
        icon: &'static str,
    },
    /// Two buttons with 50/50 split (typically cancel/save pattern)
    TwoButtons {
        /// Event emitted by left button
        left_event: Event,
        /// Icon name for left button
        left_icon: &'static str,
        /// Event emitted by right button
        right_event: Event,
        /// Icon name for right button
        right_icon: &'static str,
    },
}

/// Reusable bottom bar component for settings editor views
///
/// Provides a consistent bottom bar with white background and configurable
/// button layout. Supports single centered button or two buttons with 50/50 split.
pub struct SettingsEditorBottomBar {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl SettingsEditorBottomBar {
    /// Creates a new settings editor bottom bar
    ///
    /// # Arguments
    ///
    /// * `rect` - The rectangle defining the bottom bar's position and size
    /// * `variant` - The button layout variant to use
    ///
    /// # Returns
    ///
    /// A new `SettingsEditorBottomBar` instance
    ///
    /// # Examples
    ///
    /// ```
    /// use cadmus_core::view::settings_editor::{SettingsEditorBottomBar, BottomBarVariant};
    /// use cadmus_core::view::Event;
    /// use cadmus_core::geom::{Rectangle, Point};
    ///
    /// let rect = Rectangle::new(Point { x: 0, y: 0 }, Point { x: 100, y: 50 });
    /// let bottom_bar = SettingsEditorBottomBar::new(
    ///     rect,
    ///     BottomBarVariant::SingleButton {
    ///         event: Event::Validate,
    ///         icon: "check_mark-large",
    ///     },
    /// );
    /// ```
    pub fn new(rect: Rectangle, variant: BottomBarVariant) -> Self {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();

        let background = Filler::new(rect, WHITE);
        children.push(Box::new(background) as Box<dyn View>);

        match variant {
            BottomBarVariant::SingleButton {
                event,
                icon: icon_name,
            } => {
                let icon = Icon::new(icon_name, rect, event);
                children.push(Box::new(icon) as Box<dyn View>);
            }
            BottomBarVariant::TwoButtons {
                left_event,
                left_icon,
                right_event,
                right_icon,
            } => {
                let button_width = rect.width() as i32 / 2;

                let left_rect = rect![
                    rect.min.x,
                    rect.min.y,
                    rect.min.x + button_width,
                    rect.max.y
                ];
                let left_button = Icon::new(left_icon, left_rect, left_event);
                children.push(Box::new(left_button) as Box<dyn View>);

                let right_rect = rect![
                    rect.min.x + button_width,
                    rect.min.y,
                    rect.max.x,
                    rect.max.y
                ];
                let right_button = Icon::new(right_icon, right_rect, right_event);
                children.push(Box::new(right_button) as Box<dyn View>);
            }
        }

        SettingsEditorBottomBar { id, rect, children }
    }
}

impl View for SettingsEditorBottomBar {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?_evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        _evt: &Event,
        _hub: &crate::view::Hub,
        _bus: &mut crate::view::Bus,
        _rq: &mut crate::view::RenderQueue,
        _context: &mut crate::context::Context,
    ) -> bool {
        false
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;

    #[test]
    fn test_single_button_creates_two_children() {
        let rect = Rectangle::new(Point { x: 0, y: 0 }, Point { x: 100, y: 50 });

        let bottom_bar = SettingsEditorBottomBar::new(
            rect,
            BottomBarVariant::SingleButton {
                event: Event::Back,
                icon: "back",
            },
        );

        assert_eq!(
            bottom_bar.children().len(),
            2,
            "SingleButton variant should have 2 children: background filler and icon"
        );
    }

    #[test]
    fn test_two_buttons_creates_three_children() {
        let rect = Rectangle::new(Point { x: 0, y: 0 }, Point { x: 100, y: 50 });

        let bottom_bar = SettingsEditorBottomBar::new(
            rect,
            BottomBarVariant::TwoButtons {
                left_event: Event::Back,
                left_icon: "back",
                right_event: Event::Validate,
                right_icon: "check_mark",
            },
        );

        assert_eq!(
            bottom_bar.children().len(),
            3,
            "TwoButtons variant should have 3 children: background filler, left icon, and right icon"
        );
    }

    #[test]
    fn test_two_buttons_split_width_evenly() {
        let rect = Rectangle::new(Point { x: 0, y: 0 }, Point { x: 200, y: 50 });

        let bottom_bar = SettingsEditorBottomBar::new(
            rect,
            BottomBarVariant::TwoButtons {
                left_event: Event::Back,
                left_icon: "back",
                right_event: Event::Validate,
                right_icon: "check_mark",
            },
        );

        let children = bottom_bar.children();
        let left_button_rect = children[1].rect();
        let right_button_rect = children[2].rect();

        assert_eq!(
            left_button_rect.width(),
            100,
            "Left button should be 100 units wide"
        );
        assert_eq!(
            right_button_rect.width(),
            100,
            "Right button should be 100 units wide"
        );
        assert_eq!(left_button_rect.min.x, 0);
        assert_eq!(left_button_rect.max.x, 100);
        assert_eq!(right_button_rect.min.x, 100);
        assert_eq!(right_button_rect.max.x, 200);
    }
}
