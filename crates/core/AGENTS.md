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
[`src/input/reader.rs`](src/input/reader.rs) is the `AsyncFd` reference pattern.

Use a [`Job`](src/runtime.rs) for work whose cancellation and lifetime are owned
by the subsystem that started it. Dropping the `Job` cancels it;
`Job::join(deadline)` waits up to the deadline, aborts the job only if it has
not finished, and returns a `JobOutcome`. Detached `runtime::spawn` is allowed
only when ownership already lives elsewhere, such as a caller-held token or a
`JoinSet`. Short-lived helpers inside a job may use `tokio::spawn` when tied to
a parent `CancellationToken` (for example per-finger hold timers under
[`GesturePipeline`](src/gesture.rs)).

Device handlers are driven through the test harness; see
[Device handlers](src/device/AGENTS.md).

## Async traits

A bare `async fn` in a **public** trait warns (`async_fn_in_trait`): callers
cannot name the future's auto-trait bounds, so adding `Send` later would be a
breaking change. Pick a form by object safety first, then by `Send`:

- **The trait is used as `dyn`** — use `#[async_trait]`. Boxed `dyn Future`
  returns and `self: Arc<Self>` remain dyn-compatible. `where Self: Sized`
  excludes a method from dynamic dispatch without making the trait itself
  dyn-incompatible. If the future genuinely cannot be `Send`, as with
  single-threaded view state, use `#[async_trait(?Send)]` and say why in the
  trait's doc comment.
- **Not used as `dyn`, and the future is `Send`** — prefer stable RPITIT:
  declare `fn foo(&self) -> impl Future<Output = T> + Send;` in the trait and
  `async fn foo(..)` in the impl. The two spellings may mix; no boxing,
  dependency, or warning. See [`Fingerprint`](src/helpers.rs).
- **Not used as `dyn`, and the future cannot be `Send`** — declare
  `fn foo(&self) -> impl Future<Output = T>;` without the bound and document
  why. [`DeviceLifecycle::handle_event`](src/device/mod.rs) does this.

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
`tracing::instrument` attributes, gated behind the `tracing` feature.

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
- Use `ignore` only when the example cannot compile in the doctest harness (for
  example because it uses `crate::` paths). Add a comment explaining why.
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

Use typed macros (`sqlx::query!`, `sqlx::query_as!`, `sqlx::query_scalar!`) for
queries with a static shape; do not use untyped `sqlx::query()`,
`sqlx::query_as()`, or `sqlx::query_scalar()` for them. Untyped functions are
allowed only for statements the offline query cache cannot describe, such as
`PRAGMA` and `VACUUM`, and for dynamic SQL that typed macros cannot represent.
Document why on the function.

- When `fetch_optional` on a nullable `query_scalar!` yields
  `Option<Option<T>>`, use `.flatten()` to obtain `Option<T>`.
- Dynamic SQL paths such as runtime-selected `ORDER BY` columns need unit tests
  for every variant and a comment explaining why a typed macro cannot be used.

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
