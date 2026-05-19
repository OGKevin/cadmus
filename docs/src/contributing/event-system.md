# Event System

Cadmus uses a tree-based event system where views are organized hierarchically and events flow
through two distinct channels: the **Hub** and the **Bus**. Understanding the difference between
these channels is essential for implementing correct event handling in views.

> [!TIP]
> CSS is hard, this page might render better on 80% zoom on smaller screens.
> I tried to make the mermaid diagrams as big as possible to make them easier to read,
> which made them overflow a bit.
>
> You might also want to hide the sidebar.

## Overview

The UI is a tree of `View` objects. Each view can have children, forming a hierarchy like:

```mermaid
flowchart TD
    Home["Home (root view)"]
    TopBar["TopBar"]
    Shelf["Shelf"]
    Book1["Book"]
    Book2["Book"]
    BottomBar["BottomBar"]
    Dialog["Dialog ← overlay child"]
    Label["Label"]
    Button["Button"]

    Home --> TopBar
    Home --> Shelf
    Home --> BottomBar
    Home --> Dialog
    Shelf --> Book1
    Shelf --> Book2
    Dialog --> Label
    Dialog --> Button
```

Events enter the tree from the main loop and travel **top-down** (root to leaves), with the
highest z-level children checked first. Views can communicate back **up** the tree via the bus,
or **globally** via the hub.

## Hub vs Bus

The two channels serve fundamentally different purposes:

```mermaid
flowchart TB
    subgraph MainLoop["Main Loop"]
        rx["rx.recv()"]
        match["match evt { ... }"]
        handle["handle_event(root, &evt, &tx, &bus, ...)"]
        Hub["Hub (tx)"]
        Bus["Bus (VecDeque)"]

        rx --> match
        match -->|dispatches to view tree| handle
        handle --> Hub
        handle --> Bus
        Bus -->|unhandled bus events forwarded to hub| Hub
    end
```

### Hub (`Sender<Event>`)

The hub is an `mpsc::Sender<Event>` — a global channel that sends events to the **main loop**.
Events sent to the hub are processed in the **next iteration** of the main loop, not immediately.

**Use the hub when:**

- The event needs to be handled by the main loop directly (e.g., `Event::Close`, `Event::Open`,
  `Event::Notification`)
- The event should reach all views in a future dispatch cycle (e.g., `Event::Focus`)
- You need to communicate across unrelated parts of the view tree

```rust
// Close a view — handled by the main loop's match statement
hub.send(Event::Close(self.view_id)).ok();

// Show a notification — main loop creates the Notification view
hub.send(Event::Notification(NotificationEvent::Show(msg))).ok();

// Set focus — dispatched to all views in the next loop iteration
hub.send(Event::Focus(Some(ViewId::SearchInput))).ok();
```

### Bus (`VecDeque<Event>`)

The bus is a local `VecDeque<Event>` that passes events **from a child to its parent**. Events
placed on the bus are handled synchronously during the current dispatch cycle.

**Use the bus when:**

- A child needs to communicate with its direct parent
- The parent is expected to handle the event (e.g., a button telling its parent dialog it was
  pressed)
- The event should bubble up through the view hierarchy

```rust
// Child tells parent about a submission
bus.push_back(Event::Submit(self.view_id, self.text.clone()));

// Child requests parent to close it
bus.push_back(Event::Close(ViewId::MarginCropper));
```

### Bus Bubbling

When a bus event is not handled by any ancestor view, it reaches the root and gets **forwarded
to the hub** for processing in the next main loop iteration:

```rust
// End of main loop iteration — unhandled bus events become hub events
while let Some(ce) = bus.pop_front() {
    tx.send(ce).ok();
}
```

## Event Dispatch

The core dispatch function in `view/mod.rs` controls how events flow through the tree:

```mermaid
flowchart TD
    Start["handle_event(view, event)"] --> CheckLeaf{"Is view a leaf<br/>(no children)?"}

    CheckLeaf -->|YES| HandleDirect["call view.handle_event()<br/>return result"]
    CheckLeaf -->|NO| ReverseIter["Step 1 - Iterate children in REVERSE order<br/>(highest z-level first)"]

    ReverseIter --> CheckChildren["For each child:<br/>call handle_event(child, event)"]
    CheckChildren --> Captured{"Child returns true?"}
    Captured -->|YES| SetCaptured["captured = true<br/>BREAK"]
    Captured -->|NO| NextChild["Next child"]
    NextChild --> CheckChildren

    SetCaptured --> ProcessBus["Step 2 - Process child_bus events"]
    CheckChildren -->|No more children| ProcessBus

    ProcessBus --> BusEvent{"For each event<br/>on child_bus"}
    BusEvent --> HandleBus["call view.handle_event(child_evt)"]
    HandleBus --> ViewHandles{"View handles it?"}
    ViewHandles -->|YES| RemoveBus["remove from bus"]
    ViewHandles -->|NO| KeepBus["keep on bus<br/>(bubbles up)"]
    RemoveBus --> BusEvent
    KeepBus --> BusEvent

    BusEvent -->|No more bus events| CheckCaptured{"NOT captured<br/>by any child?"}
    CheckCaptured -->|YES| HandleParent["Step 3 - call view.handle_event(event)<br/>return captured || view's result"]
    CheckCaptured -->|NO| ReturnCaptured["return captured"]
```

### Key Rules

1. **Reverse iteration**: Children are checked from last to first (highest z-level first). This
   ensures overlays like dialogs and menus receive events before the views beneath them.

2. **Short-circuit on capture**: Once a child returns `true`, no other children or the parent
   view receive the event. This is why the Dialog's outside-tap handler works — it returns `true`
   to prevent the event from reaching views behind it.

3. **Parent only runs if uncaptured**: The parent's `handle_event` is only called if no child
   captured the event (`captured || view.handle_event(...)`). If `captured` is `true`, the
   parent is short-circuited.

## Main Loop Event Handling

The main loop (`app.rs`) receives events from the hub and handles them in two
stages. First, `TaskManager` gets a chance to observe every event — this is
where `ImportLibrary`, `ImportFinished`, and `ReindexDictionaries` are
intercepted to schedule or coalesce background tasks. Then the event enters the
large `match` statement, where some events are dispatched into the view tree
and others are handled directly:

```mermaid
flowchart TB
    subgraph MainLoop["Main Loop"]
        direction TB

        Recv["rx.recv() → evt"]
        TaskManager["TaskManager::handle_event()<br/>(ImportLibrary, ImportFinished, ReindexDictionaries)"]

        Gesture["Event::Gesture(Tap/Swipe/...)"]
        GestureAction["Dispatched into view tree via handle_event()"]

        Close["Event::Close(id)"]
        CloseAction["Handled directly:<br/>locate_by_id() + remove"]

        Notification["Event::Notification(...)"]
        NotificationAction["Handled directly:<br/>create/update Notification"]

        Open["Event::Open(info)"]
        OpenAction["Handled directly:<br/>push Reader onto history"]

        Focus["Event::Focus(...)"]
        FocusAction["Dispatched into view tree via handle_event()"]

        Select["Event::Select(...)"]
        SelectAction["Some handled directly,<br/>some dispatched"]

        ImportLibrary["Event::ImportLibrary / ImportFinished / ReindexDictionaries"]
        ImportAction["Handled by TaskManager<br/>(schedules/coalesces tasks)"]

        Recv --> TaskManager
        TaskManager --> Gesture
        TaskManager --> Close
        TaskManager --> Notification
        TaskManager --> Open
        TaskManager --> Focus
        TaskManager --> Select
        TaskManager --> ImportLibrary

        Gesture --> GestureAction
        Close --> CloseAction
        Notification --> NotificationAction
        Open --> OpenAction
        Focus --> FocusAction
        Select --> SelectAction
        ImportLibrary --> ImportAction
    end
```

### Event::Close

`Event::Close(ViewId)` is handled **directly** by the main loop using `locate_by_id()`, which
searches only the **top-level children** of the root view:

```rust
Event::Close(id) => {
    if let Some(index) = locate_by_id(view.as_ref(), id) {
        let rect = overlapping_rectangle(view.child(index));
        rq.add(RenderData::expose(rect, UpdateMode::Gui));
        view.children_mut().remove(index);
    }
}
```

**Key limitation**: The main loop can only remove **direct children of the root**. To close
nested views, either:

1. **Use the parent's ViewId** (via hub): Removes the entire parent container
2. **Use the bus**: Parent handles the close and removes just the specific child

See [Why ViewId Matters for Close](#why-viewid-matters-for-close) and
[Closing Nested Views via the Bus](#closing-nested-views-via-the-bus) for details.

## Practical Example: Dialog Outside-Tap

When a `Dialog` is tapped outside its bounds, the following sequence occurs:

```mermaid
sequenceDiagram
    actor User
    participant MainLoop as Main Loop
    participant ViewTree as View Tree
    participant Dialog as Dialog
    participant Parent as Parent View

    Note over User,Parent: 1. User taps at (x, y) outside the dialog

    User->>MainLoop: Tap gesture
    Note right of MainLoop: 2. Receives Event::Gesture(Tap(point))
    MainLoop->>ViewTree: Dispatches via handle_event()

    Note over ViewTree: 3. View tree dispatch (reverse child order)<br/>Dialog checked first (highest z-level)

    ViewTree->>Dialog: handle_event()
    Note right of Dialog: - Tap outside self.rect<br/>- Matches outside-tap arm<br/>- Sends Event::Close(self.view_id) to hub<br/>- Returns true (captured)
    Dialog-->>ViewTree: return true (captured)

    Note over Parent: 4. Parent SHORT-CIRCUITED<br/>(captured = true)<br/>Parent's handle_event() never called

    Note over MainLoop: 5. Next main loop iteration
    MainLoop->>MainLoop: Receives Event::Close(dialog_view_id)
    Note right of MainLoop: locate_by_id() finds dialog<br/>in top-level children<br/>Removes dialog from view tree
```

### Why ViewId Matters for Close

When using the **hub** to close views, `locate_by_id()` only finds top-level children. If a
dialog is nested inside a parent, you must use the **parent's** `ViewId` — this removes the
entire parent:

```mermaid
flowchart TD
    Root["Root View"]
    Home["Home"]
    OtaView["OtaView (top-level)"]
    Dialog["Dialog (nested)"]

    Root --> Home
    Root --> OtaView
    OtaView --> Dialog

    Note["hub.send(Event::Close(ViewId::Ota(Main)))<br/>→ Removes OtaView + Dialog"]
```

To close just the nested view without affecting siblings, use the **bus** instead (see below).

### Closing Nested Views via the Bus

Send `Event::Close` via the **bus** to close nested views. Parents handle bus events synchronously
and can remove any child directly:

```rust
// Child sends close via bus
bus.push_back(Event::Close(ViewId::Dialog));
```

```mermaid
sequenceDiagram
    participant Child as Button
    participant Dialog as Dialog (parent)
    participant Grandparent as OtaView

    Child->>Child: handle_event()
    Child-->>Dialog: bus: Event::Close(ViewId::Dialog)
    Dialog->>Dialog: Returns false (not handled)
    Dialog-->>Grandparent: Event bubbles up
    Grandparent->>Grandparent: Removes Dialog from children
```

**Comparison:**

| Method    | Code                              | Handler     | Scope                   |
| --------- | --------------------------------- | ----------- | ----------------------- |
| Hub close | `hub.send(Event::Close(id))`      | Main loop   | Top-level children only |
| Bus close | `bus.push_back(Event::Close(id))` | Parent view | Any child               |

**Example implementation:**

```rust
impl View for Dialog {
    fn handle_event(&mut self, evt: &Event, hub: &Sender<Event>, bus: &mut Bus, ...) -> bool {
        match *evt {
            // Return false to bubble up so grandparent removes us
            Event::Close(ViewId::Dialog) => false,
            _ => false,
        }
    }
}
```

## Summary

| Aspect           | Hub                            | Bus                             |
| ---------------- | ------------------------------ | ------------------------------- |
| Type             | `mpsc::Sender<Event>`          | `VecDeque<Event>`               |
| Scope            | Global (main loop)             | Local (parent-child)            |
| Timing           | Next loop iteration            | Current dispatch cycle          |
| Direction        | View → Main loop               | Child → Parent                  |
| Unhandled events | Processed by main loop `match` | Forwarded to hub                |
| Use for          | Close, Focus, Notifications    | Submit, child-to-parent signals |
