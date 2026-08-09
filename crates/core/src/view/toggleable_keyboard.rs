//! A reusable keyboard component that can be toggled on and off.
//!
//! `ToggleableKeyboard` encapsulates the keyboard view along with its separator,
//! providing a clean API for managing keyboard visibility in parent views.

use crate::color::BLACK;
use crate::device::AppContext;
use crate::device::DeviceIdentity as _;
use crate::framebuffer::UpdateMode;
use crate::geom::{Rectangle, halves};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::keyboard::Keyboard;
use crate::view::{BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM};
use crate::view::{Bus, Event, Hub, ID_FEEDER, Id, RenderData, RenderQueue, View};

/// A view component that wraps a keyboard and provides toggle functionality.
///
/// This component manages a keyboard view along with a separator line,
/// handling all the complexity of showing/hiding the keyboard, updating
/// the context, and managing focus events.
///
/// # Examples
///
/// ```rust,ignore
/// let keyboard = ToggleableKeyboard::new(parent_rect, false);
/// children.push(Box::new(keyboard) as Box<dyn View>);
///
/// // Later, to toggle keyboard visibility:
/// if let Some(index) = locate::<ToggleableKeyboard>(self) {
///     let kb = self.children[index].downcast_mut::<ToggleableKeyboard>().unwrap();
///     kb.toggle(hub, rq, context);  // Toggles between hidden/visible
/// }
/// ```
pub struct ToggleableKeyboard {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    visible: bool,
    parent_rect: Rectangle,
    number_mode: bool,
    layout: Option<String>,
    has_bottom_bar: bool,
}

impl ToggleableKeyboard {
    /// Creates a new `ToggleableKeyboard` instance.
    ///
    /// The keyboard is initially hidden and must be explicitly shown
    /// by calling `toggle(...)` or `set_visible(true, ...)`.
    ///
    /// # Arguments
    ///
    /// * `parent_rect` - The rectangle of the parent view, used for positioning
    /// * `number_mode` - If `true`, the keyboard starts in number mode
    ///
    /// # Returns
    ///
    /// A new `ToggleableKeyboard` instance in hidden state.
    pub fn new(parent_rect: Rectangle, number_mode: bool) -> Self {
        ToggleableKeyboard {
            id: ID_FEEDER.next(),
            rect: Rectangle::default(),
            children: Vec::new(),
            visible: false,
            parent_rect,
            number_mode,
            layout: None,
            has_bottom_bar: true,
        }
    }

    /// Sets a custom keyboard layout to use instead of the default.
    ///
    /// When set, this layout will be used whenever the keyboard is shown,
    /// temporarily overriding `context.settings.keyboard_layout`.
    ///
    /// # Arguments
    ///
    /// * `layout` - The name of the keyboard layout (e.g., "Terminal")
    pub fn with_layout(mut self, layout: impl Into<String>) -> Self {
        self.layout = Some(layout.into());
        self
    }

    /// Configures whether to reserve space for a bottom bar below the keyboard.
    ///
    /// By default, the keyboard reserves space for a bottom bar (e.g., status bar).
    /// Set this to `false` for full-screen views without a bottom bar.
    ///
    /// # Arguments
    ///
    /// * `has_bottom_bar` - `true` to reserve space, `false` for keyboard at screen bottom
    pub fn with_bottom_bar(mut self, has_bottom_bar: bool) -> Self {
        self.has_bottom_bar = has_bottom_bar;
        self
    }

    /// Toggles the keyboard visibility between hidden and visible.
    ///
    /// If the keyboard is currently hidden, it will be shown.
    /// If the keyboard is currently visible, it will be hidden.
    /// When hiding, this clears focus and updates the context.
    ///
    /// # Arguments
    ///
    /// * `hub` - Event hub for sending focus events
    /// * `rq` - Render queue for scheduling redraws
    /// * `context` - Application context for updating keyboard state
    pub fn toggle(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut AppContext) {
        if self.visible {
            self.hide(hub, rq, context);
        } else {
            self.show(rq, context);
        }
    }

    /// Sets the keyboard visibility to the specified state.
    ///
    /// This is more explicit than `toggle()` when you know whether you want
    /// to show or hide the keyboard. If the keyboard is already in the desired
    /// state, this is a no-op.
    ///
    /// # Arguments
    ///
    /// * `visible` - `true` to show the keyboard, `false` to hide it
    /// * `hub` - Event hub for sending focus events
    /// * `rq` - Render queue for scheduling redraws
    /// * `context` - Application context for updating keyboard state
    pub fn set_visible(
        &mut self,
        visible: bool,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        if self.visible == visible {
            return;
        }

        if visible {
            self.show(rq, context);
        } else {
            self.hide(hub, rq, context);
        }
    }

    /// Sets the keyboard number mode.
    ///
    /// When number mode is enabled, the keyboard displays numbers and
    /// symbols instead of letters. This setting only takes effect the
    /// next time the keyboard is shown.
    ///
    /// # Arguments
    ///
    /// * `number_mode` - `true` to enable number mode, `false` for letter mode
    pub fn set_number_mode(&mut self, number_mode: bool) {
        self.number_mode = number_mode;
    }

    /// Returns whether the keyboard is currently visible.
    ///
    /// # Returns
    ///
    /// `true` if the keyboard is visible, `false` otherwise.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Returns the height the keyboard will occupy when visible.
    ///
    /// This is useful for layout calculations before showing the keyboard.
    /// The height includes the separator line and all keyboard rows.
    /// If `has_bottom_bar` is true, includes space for a bottom bar.
    pub fn keyboard_height(&self, context: &AppContext) -> i32 {
        let dpi = context.device.dpi();
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let big_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        if self.has_bottom_bar {
            small_height + 3 * big_height + thickness
        } else {
            3 * big_height + thickness
        }
    }

    /// Shows the keyboard by creating the separator and keyboard views.
    ///
    /// This method calculates the proper positioning based on the parent rect
    /// and creates both the separator line and the keyboard itself.
    fn show(&mut self, rq: &mut RenderQueue, context: &mut AppContext) {
        let dpi = context.device.dpi();
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (small_thickness, big_thickness) = halves(thickness);

        let keyboard_height = self.keyboard_height(context);
        let bottom_offset = if self.has_bottom_bar {
            small_height + big_thickness
        } else {
            small_thickness
        };

        let separator = Filler::new(
            rect![
                self.parent_rect.min.x,
                self.parent_rect.max.y - keyboard_height,
                self.parent_rect.max.x,
                self.parent_rect.max.y - keyboard_height + thickness
            ],
            BLACK,
        );
        self.children.push(Box::new(separator) as Box<dyn View>);

        let mut kb_rect = rect![
            self.parent_rect.min.x,
            self.parent_rect.max.y - keyboard_height + big_thickness,
            self.parent_rect.max.x,
            self.parent_rect.max.y - bottom_offset
        ];

        let saved_layout = self.layout.as_ref().map(|layout| {
            let saved = context.settings.keyboard_layout.clone();
            context.settings.keyboard_layout = layout.clone();
            saved
        });

        let keyboard = Keyboard::new(&mut kb_rect, self.number_mode, context);
        self.children.push(Box::new(keyboard) as Box<dyn View>);

        if let Some(saved) = saved_layout {
            context.settings.keyboard_layout = saved;
        }

        self.rect = kb_rect;
        self.rect.absorb(self.children[0].rect());

        self.visible = true;

        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
    }

    /// Hides the keyboard by clearing all child views and resetting state.
    ///
    /// This method also clears the focus and updates the context to reflect
    /// that no keyboard is active.
    fn hide(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut AppContext) {
        let rect = self.rect;

        self.children.clear();

        context.kb_rect = Rectangle::default();
        self.rect = Rectangle::default();
        self.visible = false;

        hub.send(Event::Focus(None)).ok();
        rq.add(RenderData::expose(rect, UpdateMode::Gui));
    }
}

impl View for ToggleableKeyboard {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        if !self.visible {
            return false;
        }

        for child in &mut self.children {
            if child.handle_event(evt, hub, bus, rq, context) {
                return true;
            }
        }

        false
    }
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context), fields(rect = ?rect)))]
    fn render(&self, context: &mut AppContext, rect: Rectangle) {
        if !self.visible {
            return;
        }

        for child in &self.children {
            child.render(context, rect);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use std::sync::mpsc::channel;

    fn create_test_keyboard() -> ToggleableKeyboard {
        let parent_rect = rect![0, 0, 600, 800];
        ToggleableKeyboard::new(parent_rect, false)
    }

    fn create_test_context_with_keyboard_data() -> AppContext {
        let mut context = create_test_context();
        context.load_keyboard_layouts();
        context.load_dictionaries();
        context
    }

    #[test]
    fn test_new_creates_hidden_keyboard() {
        let keyboard = create_test_keyboard();
        assert!(!keyboard.is_visible());
        assert_eq!(keyboard.children.len(), 0);
        assert!(!keyboard.number_mode);
    }

    #[test]
    fn test_new_with_number_mode() {
        let parent_rect = rect![0, 0, 600, 800];
        let keyboard = ToggleableKeyboard::new(parent_rect, true);
        assert!(keyboard.number_mode);
    }

    #[test]
    fn test_is_visible_initially_false() {
        let keyboard = create_test_keyboard();
        assert!(!keyboard.is_visible());
    }

    #[test]
    fn test_set_number_mode() {
        let mut keyboard = create_test_keyboard();
        assert!(!keyboard.number_mode);

        keyboard.set_number_mode(true);
        assert!(keyboard.number_mode);

        keyboard.set_number_mode(false);
        assert!(!keyboard.number_mode);
    }

    #[test]
    fn test_rect_defaults_to_empty() {
        let keyboard = create_test_keyboard();
        let rect = keyboard.rect();
        assert_eq!(rect.min.x, 0);
        assert_eq!(rect.min.y, 0);
        assert_eq!(rect.max.x, 0);
        assert_eq!(rect.max.y, 0);
    }

    #[test]
    fn test_children_empty_when_hidden() {
        let keyboard = create_test_keyboard();
        assert!(keyboard.children().is_empty());
    }

    #[test]
    fn test_parent_rect_stored_correctly() {
        let parent_rect = rect![10, 20, 590, 780];
        let keyboard = ToggleableKeyboard::new(parent_rect, false);
        assert_eq!(keyboard.parent_rect, parent_rect);
    }

    #[test]
    fn test_with_layout_sets_custom_layout() {
        let parent_rect = rect![0, 0, 600, 800];
        let keyboard = ToggleableKeyboard::new(parent_rect, false).with_layout("Terminal");
        assert_eq!(keyboard.layout, Some("Terminal".to_string()));
    }

    #[test]
    fn test_default_layout_is_none() {
        let keyboard = create_test_keyboard();
        assert!(keyboard.layout.is_none());
    }

    #[test]
    fn test_default_has_bottom_bar_is_true() {
        let keyboard = create_test_keyboard();
        assert!(keyboard.has_bottom_bar);
    }

    #[test]
    fn test_with_bottom_bar_false() {
        let parent_rect = rect![0, 0, 600, 800];
        let keyboard = ToggleableKeyboard::new(parent_rect, false).with_bottom_bar(false);
        assert!(!keyboard.has_bottom_bar);
    }

    #[test]
    fn test_with_bottom_bar_true() {
        let parent_rect = rect![0, 0, 600, 800];
        let keyboard = ToggleableKeyboard::new(parent_rect, false).with_bottom_bar(true);
        assert!(keyboard.has_bottom_bar);
    }

    #[test]
    fn test_keyboard_height_with_bottom_bar() {
        let parent_rect = rect![0, 0, 600, 800];
        let context = create_test_context();
        let keyboard_with_bar = ToggleableKeyboard::new(parent_rect, false).with_bottom_bar(true);
        let keyboard_without_bar =
            ToggleableKeyboard::new(parent_rect, false).with_bottom_bar(false);

        let height_with_bar = keyboard_with_bar.keyboard_height(&context);
        let height_without_bar = keyboard_without_bar.keyboard_height(&context);

        assert!(height_with_bar > height_without_bar);
        assert!(height_with_bar > 0);
        assert!(height_without_bar > 0);
    }

    #[test]
    fn test_builder_chaining() {
        let parent_rect = rect![0, 0, 600, 800];
        let keyboard = ToggleableKeyboard::new(parent_rect, false)
            .with_layout("Terminal")
            .with_bottom_bar(false);

        assert_eq!(keyboard.layout, Some("Terminal".to_string()));
        assert!(!keyboard.has_bottom_bar);
    }

    #[test]
    fn test_toggle_from_hidden_shows_keyboard() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        assert!(!keyboard.is_visible());
        assert!(keyboard.children.is_empty());
        assert!(rq.is_empty());

        keyboard.toggle(&hub, &mut rq, &mut context);

        assert!(keyboard.is_visible());
        assert_eq!(keyboard.children.len(), 2);
        assert_eq!(rq.len(), 1);
    }

    #[test]
    fn test_toggle_from_visible_hides_keyboard() {
        let mut keyboard = create_test_keyboard();
        let (hub, receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        keyboard.toggle(&hub, &mut rq, &mut context);
        assert!(keyboard.is_visible());
        assert_eq!(keyboard.children.len(), 2);

        rq = RenderQueue::new();
        keyboard.toggle(&hub, &mut rq, &mut context);

        assert!(!keyboard.is_visible());
        assert!(keyboard.children.is_empty());
        assert_eq!(rq.len(), 1);
        assert_eq!(context.kb_rect, Rectangle::default());

        let focus_event = receiver.try_recv().unwrap();
        assert!(matches!(focus_event, Event::Focus(None)));
    }

    #[test]
    fn test_toggle_twice_returns_to_original_state() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        keyboard.toggle(&hub, &mut rq, &mut context);
        keyboard.toggle(&hub, &mut rq, &mut context);

        assert!(!keyboard.is_visible());
        assert!(keyboard.children.is_empty());
    }

    #[test]
    fn test_toggle_adds_render_data_each_time() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut context = create_test_context_with_keyboard_data();

        let mut rq = RenderQueue::new();
        keyboard.toggle(&hub, &mut rq, &mut context);
        assert_eq!(rq.len(), 1);

        let mut rq = RenderQueue::new();
        keyboard.toggle(&hub, &mut rq, &mut context);
        assert_eq!(rq.len(), 1);
    }

    #[test]
    fn test_set_visible_true_shows_keyboard() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        assert!(!keyboard.is_visible());
        assert!(keyboard.children.is_empty());

        keyboard.set_visible(true, &hub, &mut rq, &mut context);

        assert!(keyboard.is_visible());
        assert_eq!(keyboard.children.len(), 2);
        assert_eq!(rq.len(), 1);
    }

    #[test]
    fn test_set_visible_false_hides_keyboard() {
        let mut keyboard = create_test_keyboard();
        let (hub, receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        keyboard.set_visible(true, &hub, &mut rq, &mut context);
        assert!(keyboard.is_visible());
        assert_eq!(keyboard.children.len(), 2);

        rq = RenderQueue::new();
        keyboard.set_visible(false, &hub, &mut rq, &mut context);

        assert!(!keyboard.is_visible());
        assert!(keyboard.children.is_empty());
        assert_eq!(rq.len(), 1);
        assert_eq!(context.kb_rect, Rectangle::default());

        let focus_event = receiver.try_recv().unwrap();
        assert!(matches!(focus_event, Event::Focus(None)));
    }

    #[test]
    fn test_set_visible_noop_when_already_visible() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        keyboard.set_visible(true, &hub, &mut rq, &mut context);
        assert!(keyboard.is_visible());

        rq = RenderQueue::new();
        keyboard.set_visible(true, &hub, &mut rq, &mut context);

        assert!(keyboard.is_visible());
        assert!(rq.is_empty());
    }

    #[test]
    fn test_set_visible_noop_when_already_hidden() {
        let mut keyboard = create_test_keyboard();
        let (hub, _receiver) = channel();
        let mut rq = RenderQueue::new();
        let mut context = create_test_context_with_keyboard_data();

        assert!(!keyboard.is_visible());

        keyboard.set_visible(false, &hub, &mut rq, &mut context);

        assert!(!keyboard.is_visible());
        assert!(rq.is_empty());
    }
}
