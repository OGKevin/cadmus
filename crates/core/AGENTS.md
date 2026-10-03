# Core Crate — Agent Coding Conventions

## See also

- [Background task stop/join policy](src/task/AGENTS.md)
- [Device handlers and the test harness](src/device/AGENTS.md)
- [WiFi leases](src/device/wifi/AGENTS.md)
- [UI string translations](i18n/AGENTS.md)

## Async runtime and Jobs

The process runtime is built by [`runtime::builder`](src/runtime.rs): one
multi-thread Tokio runtime with the worker pool capped at `WORKER_THREADS` and
the blocking pool capped at `MAX_BLOCKING_THREADS`. The main app future runs on
the main thread; spawned tasks run on the worker pool. Blocking a worker stalls
input and timers, so use an async API or `spawn_blocking` there.
[`src/input.rs`](src/input.rs) is the `AsyncFd` reference pattern.

Work that outlives the current call — input pipelines, view polling, downloads,
timers — must be a [`Job`](src/runtime.rs) owned by the subsystem that started
it, not a bare `runtime::current_handle().spawn`. Dropping the `Job` cancels and
detaches; `Job::join` awaits completion. Short-lived helpers inside an existing
job may use `tokio::spawn` when they are tied to a parent `CancellationToken`
(for example per-finger hold timers under [`GesturePipeline`](src/gesture.rs)).

Device handlers are driven through the test harness; see
[Device handlers](src/device/AGENTS.md).

## Async traits

A bare `async fn` in a **public** trait warns (`async_fn_in_trait`): callers
cannot name the future's auto-trait bounds, so adding `Send` later would be a
breaking change. Pick a form by object safety first, then by `Send`:

- **The trait is used as `dyn`** — `#[async_trait]`, the only form that stays
  object-safe. A `!Send` default method (boxed future, `self: Arc<Self>`, or a
  `where Self: Sized` bound) also breaks dyn-compatibility, so dyn traits get no
  such helper.
- **Not used as `dyn`, and the future is `Send`** — prefer the stable RPITIT
  form: declare `fn foo(&self) -> impl Future<Output = T> + Send;` in the trait
  and `async fn foo(..)` in the impl. The two spellings may mix; no boxing, no
  dependency, no warning. [`Fingerprint`](src/helpers.rs) and
  [`DeviceLifecycle`](src/device/mod.rs) do this. `#[trait_variant::make(Name:
  Send)]` is the alternative when an unboxed bound from a macro is wanted; it is
  not currently a workspace dependency and can be re-added when a use case
  appears. It cannot desugar a default body, so those methods need an explicit
  no-op in each impl.
- **The future genuinely cannot be `Send`**, as with single-threaded view state —
  `#[async_trait(?Send)]`, and say why in the trait's doc comment.

## Return Types

Prefer meaningful enums or `Result<T, E>` over `bool` when a function can
succeed in more than one way or fail for distinct reasons (for example a sysfs
write that was applied, skipped because the path is missing, or failed with
I/O). Leave logging and recovery policy to the caller.

```rust
// ✅ Good — caller decides how to treat Missing vs Io
fn write_sysfs(path: &Path, value: &str) -> Result<SysfsWrite, SysfsWriteError>;

// ❌ Bad — bool collapses distinct outcomes; logging is buried in the helper
fn write_sysfs(path: &Path, value: &str) -> bool;
```

## View Instrumentation

All `handle_event` and `render` methods in view components must have
OpenTelemetry tracing attributes, gated behind the `tracing` feature.

### `handle_event`

```rust
#[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
async fn handle_event(&mut self, evt: &Event, hub: &Hub, bus: &mut Bus, rq: &mut RenderQueue, context: &mut AppContext) -> bool {
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

When the message ID is only known at runtime (config, plugin payload, table of
keys), use `fl_or!(id, "English fallback")` or `fl_or!(id, "fallback", var =
value)` for Fluent placeholders (`use crate::fl_or;`). Prefer
`fl!` whenever the ID is a literal so compile-time validation still applies.
The fallback string is a last resort for an unknown ID, not a substitute for
missing Crowdin translations (those fall back to `en-GB` via the loader).

Message IDs, sort order, and locale-editing rules live in
[`i18n/AGENTS.md`](i18n/AGENTS.md). Pass parameters with
`fl!("id", var = value)`.

### Where this applies

- `label()` implementations on `SettingKind` traits
- Input field `label` strings in `Event::OpenNamedInput`
- Menu entry text (`EntryKind::Command`, `EntryKind::RadioButton`, etc.)
- Button labels, notification text, any other user-visible string
