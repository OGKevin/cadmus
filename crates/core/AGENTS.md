# Core Crate — Agent Coding Conventions

## View Instrumentation

All `handle_event` and `render` methods in view components must have
OpenTelemetry tracing attributes, gated behind the `tracing` feature.

### `handle_event`

```rust
#[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
fn handle_event(&mut self, evt: &Event, hub: &Hub, bus: &mut Bus, rq: &mut RenderQueue, context: &mut AppContext) -> bool {
```

### `render`

```rust
#[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context, _rect), fields(rect = ?_rect)))]
fn render(&self, context: &mut AppContext, _rect: Rectangle) {
```

### Rules

- `skip()` names must exactly match parameter names, including underscore
  prefixes (e.g. `skip(self, _hub, _bus, _rq, _context)` when params are
  `_hub`, `_bus`, etc.).
- Verify with `cargo check --features tracing`.

## View Rendering

`render()` is called **only** when:

- The view has no children (`view.len() == 0`), **or**
- The view is a background view (`view.is_background() == true`).

Container views with children do **not** have their `render()` called.

### Adding decoration to a container

**Option 1 — Background view**: Return `true` from `is_background()`. Renders
**before** children (suitable for backgrounds, not overlays).

**Option 2 — Child view**: Add a dedicated child view for the decoration.
Renders in child order (last child on top). Use this for overlays.

## Rustdoc Examples

Preference order: fully compilable > `no_run` > `ignore`.

- Use `no_run` when the example compiles but cannot execute (file I/O, network,
  database).
- Use `ignore` **only** for private/`pub(crate)` items unreachable from the
  test harness. Add a comment explaining why.
- Use `#` to hide boilerplate setup lines.
- Verify with `cargo test --doc`.

## Test Context

Tests must not redefine `create_test_context`. Use the shared helper:

```rust
use crate::context::test_helpers::create_test_context;
```

Tests may wrap it for additional setup but must not reimplement base `Context`
construction.

## SQL: Explicit Columns

All SQL queries must list explicit column names. Do not use `SELECT *`.

## SQL: Migrations

### Timestamps

Store all date/time values as **Unix epoch seconds** (`INTEGER NOT NULL`).
Never use `TEXT` for timestamps.

```sql
-- ✅ Good
created_at INTEGER NOT NULL
added_at   INTEGER NOT NULL DEFAULT (unixepoch('now'))
```

### Indices

Every index must be actively used by at least one query. Remove unused indices
when the query that used them is removed.

## SQL: Query Macros

Use typed macros (`sqlx::query!`, `sqlx::query_as!`, `sqlx::query_scalar!`).
Never use untyped `sqlx::query()` / `sqlx::query_as()` / `sqlx::query_scalar()`.

- Use `.flatten()` on `query_scalar!` results for nullable columns
  (`Option<Option<T>>` → `Option<T>`).

### Exception

Untyped queries are allowed for dynamic SQL (e.g. runtime `ORDER BY` column)
**only if** the function has unit tests covering every dynamic path, with a
comment explaining why the typed macro cannot be used.

## User-Facing String Translations

All user-visible strings must use the `fl!` macro (`use crate::fl;`). Never
hardcode string literals for labels, buttons, placeholders, or notifications.

### Translation rules

1. Every user-visible string needs a Fluent message ID in
   `crates/core/i18n/en-GB/cadmus_core.ftl`.
2. Use `fl!("message-id")` at the call site.
3. For parameterised strings, use Fluent variables (`{ $var }`) in the `.ftl`
   file and pass values via `fl!("id", var = value)`.
4. Keep `.ftl` keys sorted alphabetically within each comment section.
5. Naming convention — kebab-case, prefixed by feature area:
   - `settings-<category>-<description>` for settings labels
   - `settings-<category>-<description>-input` for input fields
   - `notification-<description>` for notifications

### Where this applies

- `label()` implementations on `SettingKind` traits
- Input field `label` strings in `Event::OpenNamedInput`
- Menu entry text (`EntryKind::Command`, `EntryKind::RadioButton`, etc.)
- Button labels, notification text, any other user-visible string
