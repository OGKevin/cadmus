---
description: "View rendering patterns and when render() is called"
applyTo: "crates/core/src/view/**/*.rs"
---

# View Rendering Pattern

## When is `render()` called?

A view's `render()` method is **only called** when:

- The view has **no children** (`view.len() == 0`), OR
- The view is marked as a **background view** (`view.is_background() == true`)

See `crates/core/src/view/mod.rs`, the `render()` function (lines 216-298):

```rust
if view.len() == 0 || view.is_background() {
    // View's render() method is called here
    view.render(fb, rect, fonts);
} else {
    // View has children: only children are rendered, not the parent
    bgs.extend(ids.get(&view.id()).cloned().into_iter().flatten());
}
```

## Implications

### Container Views with Children

If a view has children, its `render()` method is **not called** during the normal rendering pass. The framework only renders the children.

**Examples of container views:**

- `Toggle` - has 3 children (Label, Filler, Label)
- `DirectoriesBar` - has multiple child Directory views
- `Home` - has navigation bar, address bar, and other children

### Leaf Views without Children

If a view has no children, its `render()` method **is called** and must draw itself.

**Examples of leaf views:**

- `Label` - renders text
- `Filler` - renders a solid color rectangle
- `Icon` - renders an icon image
- `Directory` - renders directory name with optional selection box

## Pattern: Adding Visual Decoration to Container Views

When a container view needs to draw something (like a selection indicator), you have two options:

### Option 1: Mark as Background View (Simplest)

Implement `is_background()` to return `true`, which allows the view's `render()` to be called even with children.

```rust
impl View for Toggle {
    fn is_background(&self) -> bool {
        true  // Allows render() to be called even with children
    }

    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        // Draw decoration on top of children
        if self.enabled {
            // Draw selection box around first label
            fb.draw_rounded_rectangle_with_border(...);
        } else {
            // Draw selection box around second label
            fb.draw_rounded_rectangle_with_border(...);
        }
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children  // Still has children
    }
}
```

**When to use this:**

- When the container needs simple visual decoration **behind** its children
- When you want to keep all rendering logic in one place
- Works for backgrounds that render before children

**Examples:**

- Background containers that fill their area with a color
- Panels with background decorations
- **NOT for overlays** - background views render _before_ children, not after

**Important caveat:**

Background views render **before** children (lines 228-260 in `mod.rs`), so children will render on top and may overwrite the background. This pattern is suitable for backgrounds, but **not** for overlays or decorations that need to appear on top of children.

### Option 2: Add a Child View (More Modular)

Create a dedicated child view that renders the decoration.

**Example: Toggle with selection box**

```rust
pub struct Toggle {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,  // Includes SelectionBox child
    enabled: bool,
    event: Event,
}

impl Toggle {
    pub fn new(...) -> Toggle {
        let mut children = Vec::new();

        // Add label children
        children.push(Box::new(left_label));
        children.push(Box::new(separator));
        children.push(Box::new(right_label));

        // Add selection box child (renders on top of labels)
        children.push(Box::new(SelectionBox::new(rect, enabled)));

        Toggle { id, rect, children, enabled, event }
    }

    fn update_selection(&mut self) {
        // Update the SelectionBox child to show/hide or reposition
        if let Some(selection_box) = self.children[3].downcast_mut::<SelectionBox>() {
            selection_box.set_enabled(self.enabled);
        }
    }
}

// SelectionBox is a leaf view with no children
pub struct SelectionBox {
    id: Id,
    rect: Rectangle,
    enabled: bool,
    selected_rect: Rectangle,
}

impl View for SelectionBox {
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        // IMPORTANT: Use the passed 'rect' parameter, not self.rect
        // The 'rect' parameter represents the dirty region that needs to be redrawn

        // Check if the dirty region intersects with our target area
        let render_rect = rect.intersection(&self.target_rect);
        if render_rect.is_none() {
            return;
        }

        // Draw using self.target_rect (our logical bounds)
        fb.draw_rectangle_outline(&self.target_rect, ...);
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &Vec::new()
    }
}
```

**Why this works:**

- Toggle has children, so its `render()` is not called by default
- SelectionBox has no children, so its `render()` IS called
- SelectionBox renders on top of other children (rendered last due to z-order)

**When to use this:**

- When you need decoration that renders **on top** of children (overlays)
- When you need complex decoration logic that benefits from being in a separate component
- When the decoration needs its own state or event handling
- When you want maximum modularity

**Examples:**

- `Toggle` - SelectionBox child draws selection box on top of labels (see `crates/core/src/view/toggle.rs`)
- Overlay indicators that appear on top of content
- Decorations that need to render after children

## Summary

- **Container views (with children):** `render()` is NOT called by default
- **Leaf views (no children):** `render()` IS called
- **Background views:** `render()` IS called even with children
- **To add decoration to a container:** Add a dedicated child view for the decoration
- **Z-order matters:** Last child renders on top
