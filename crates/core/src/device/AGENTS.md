# Device handlers (`device/`)

Most device lifecycle handlers are async and take `&mut AppContext` and
`&mut DeviceRuntime`; `on_shutdown` instead takes a `ShutdownContext` and the
device-task slice. In tests, await async handlers through the
[`test_harness`](test_harness.rs) helpers. There is no `runtime::block_on`
helper; `runtime::enter` belongs only at process entry.

## Test harness

- [`DeviceRuntimeHarness`](test_harness.rs) builds the context (and, where
  needed, the database/library) a handler test starts from.
- [`poll_parts!`](test_harness.rs) and
  [`poll_parts_with_background!`](test_harness.rs) await a handler future that
  needs a fresh `DeviceRuntime` borrow. They expand at the call site so the
  `DeviceRuntime` they build and the future that borrows it share one scope;
  the `_with_background` form also serves tasks the handler spawns.
- [`poll_runtime_only!`](test_harness.rs) is the runtime-only variant for
  handlers that do not touch the hub, bus, or render queue.
- Synchronous handlers use [`DeviceRuntimeHarness::with_parts`](test_harness.rs),
  or `with_runtime_only` when they do not need the hub, bus, or render queue.
- [`DeviceRuntimeHarness::run_on_shutdown`](test_harness.rs) drives
  [`DeviceLifecycle::on_shutdown`](mod.rs) and is async; it must be `.await`ed.
- Device handler unit tests are `#[tokio::test]` async fns.

See the crate-level [`AGENTS.md`](../../AGENTS.md) for the runtime cost model and
`Job` ownership rule.
