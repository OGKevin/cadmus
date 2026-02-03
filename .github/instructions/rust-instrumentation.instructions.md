---
description: "Rust instrumentation guidelines for tracing and observability"
applyTo: "**/*.rs"
---

# Rust Instrumentation Guidelines

## Tracing and Observability

All `handle_event` and `render` methods in view components must be instrumented with the OpenTelemetry tracing attribute to enable proper observability and debugging.

### Required Attributes

#### handle_event Method

Add the following attribute before every `handle_event` method implementation in View traits:

```rust
#[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
fn handle_event(
    &mut self,
    evt: &Event,
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut Context,
) -> bool {
    // implementation
}
```

#### render Method

Add the following attribute before every `render` method implementation in View traits:

```rust
#[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts), fields(rect = ?rect)))]
fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
    // implementation
}
```

### Key Points

- **Conditional Compilation**: The attribute is only active when the `otel` feature is enabled, ensuring zero overhead in production builds without observability
- **Event Field Capture**: `fields(event = ?evt)` captures the event type in traces for debugging event flow
- **Rect Field Capture**: `fields(rect = ?rect)` captures the rendering rectangle for debugging layout issues
- **Selective Skipping**: Skip only large data structures (self, hub, bus, rq, context, fb, fonts) while capturing critical information (event, rect)
- **Return Value Tracing**: The `ret(level=tracing::Level::TRACE)` logs the return value at TRACE level for debugging
- **Applies to All Views**: Every view component's `handle_event` and `render` methods must have these attributes
- **Unused Parameters**: Keep unused parameter prefixes (e.g., `_hub`, `_bus`) to avoid compiler warnings. The instrumentation `skip()` directive must use the exact parameter names, including underscores (e.g., `skip(self, _hub, _bus, _rq, _context)` when parameters are named `_hub`, `_bus`, `_rq`, `_context`).
- **Verification**: Always validate instrumentation with `cargo check --features otel` to ensure the tracing macro resolves parameter names correctly.

### Examples

**Good (all parameters used):**

```rust
impl View for Button {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(center) => {
                bus.push_back(self.event.clone());
                true
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, fb, fonts), fields(rect = ?rect)))]
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts) {
        // rendering implementation
    }
}
```

**Good (with unused parameters):**

```rust
impl View for Filler {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        // No implementation needed - all params unused
        false
    }

    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {
        // No rendering needed - all params unused
    }
}
```

**Bad (missing instrumentation):**

```rust
impl View for Button {
    fn handle_event(  // Missing instrumentation attribute
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        // implementation
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        // Missing instrumentation attribute
    }
}
```

**Bad (incorrect skip directive with mismatched names):**

```rust
impl View for Filler {
    // Wrong: skip() uses names that do not match the parameters
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        false
    }
}
```

### Rationale

Instrumenting `handle_event` methods provides:

- Complete event flow tracing through the UI hierarchy
- Event type visibility for debugging interactions
- Performance profiling of event handling
- Return value tracking for understanding event consumption

Instrumenting `render` methods provides:

- Rendering performance analysis
- Identification of rendering bottlenecks
- Frame time profiling for smooth UI

All view components process events through `handle_event` and update UI through `render`, making them critical chokepoints for observability.
