---
description: "Ensure event system documentation stays in sync with app.rs changes"
applyTo: "crates/cadmus/src/app.rs,crates/core/src/view/mod.rs,crates/core/src/view/common.rs,docs/src/contributing/event-system.md"
---

# Event System Documentation Synchronization

## Purpose

Ensure that changes to the main event loop in `crates/cadmus/src/app.rs` are properly documented in `docs/src/contributing/event-system.md`.

## When This Applies

This instruction MUST be followed when modifying any of the following in `crates/cadmus/src/app.rs`:

1. **Event handling patterns** in the main loop (`while let Ok(evt) = rx.recv()`)
2. **Event::Close** handling logic
3. **Event dispatch behavior** - how different event types are routed
4. **Hub vs Bus usage** - changes to how `tx.send()` vs `bus.push_back()` are used
5. **locate_by_id** usage patterns
6. **View lifecycle management** - how views are created, removed, or transitioned

## Required Checks

Before submitting changes to `app.rs`, verify that the event system documentation reflects:

### 1. Event Handling Behavior

- [ ] If new event types are added to the main loop match statement, document their dispatch behavior
- [ ] If existing event handling changes (e.g., `Event::Close`), update the relevant sections
- [ ] If events are now dispatched to view tree vs handled directly, update the "Main Loop Event Handling" section

### 2. Event::Close Changes

The `Event::Close(id)` handling is critical. Check:

- [ ] Does it still use `locate_by_id()` to find the view?
- [ ] Does it still only search top-level children?
- [ ] Are there new patterns for closing nested views via bus?

Update these sections if changed:

- "Event::Close" subsection under "Main Loop Event Handling"
- "Why ViewId Matters for Close" subsection
- "Closing Nested Views via the Bus" subsection

### 3. Hub vs Bus Patterns

- [ ] If new patterns for hub (`tx.send()`) vs bus (`bus.push_back()`) usage emerge, document them
- [ ] Update the comparison table in the "Summary" section if needed

### 4. View Lifecycle Changes

- [ ] If view creation/removal patterns change, update relevant examples
- [ ] If `overlapping_rectangle` or `RenderData::expose` usage changes, document it

## Review Checklist

When reviewing changes to `app.rs`:

- [ ] Identify all event handling changes
- [ ] Check if `locate_by_id` usage patterns changed
- [ ] Verify hub vs bus usage is still accurate
- [ ] Update or add Mermaid diagrams if flow changes
- [ ] Ensure code examples in docs match actual implementation
- [ ] Test that documentation builds and renders correctly
