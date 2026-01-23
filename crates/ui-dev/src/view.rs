use cadmus_core::context::Context;
use cadmus_core::geom::Rectangle;
use cadmus_core::view::label::Label;
use cadmus_core::view::{Align, Hub, RenderQueue, View};

/// Create the root view to be rendered in the UI development emulator.
///
/// This function is called once at startup. Modify it to test your own components.
///
/// # Example: Creating a simple label
///
/// ```no_run
/// Box::new(Label::new(
///     rect,
///     "Hello, Plato UI Dev!".to_string(),
///     Align::Center,
/// ))
/// ```
///
/// # Example: Creating a container with children
///
/// ```no_run
/// let mut container = Filler::new(rect, Color::White);
/// // Add children to container.children_mut()
/// Box::new(container)
/// ```
///
/// # Available View Components
///
/// Some commonly used view components you can import and use:
/// - `Label`: Simple text label
/// - `Button`: Clickable button
/// - `Filler`: Colored rectangle container
/// - `Icon`: Icon display
/// - `Slider`: Value slider
///
/// Check `crates/core/src/view/` for more components.
pub fn create_root_view(
    rect: Rectangle,
    _hub: &Hub,
    _rq: &mut RenderQueue,
    _context: &mut Context,
) -> Box<dyn View> {
    Box::new(Label::new(
        rect,
        "Hello, Plato UI Dev!".to_string(),
        Align::Center,
    ))
}
