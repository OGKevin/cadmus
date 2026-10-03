# Cadmus binary (`cadmus`)

Entry goes through [`cadmus_core::runtime::enter`](../core/src/runtime.rs),
which builds the capped multi-thread runtime and bounds shutdown. Do not add
`#[tokio::main]`: the app future runs on the **main thread** so exit can apply
`SHUTDOWN_DEADLINE` after it returns. Blocking the main thread freezes the UI;
see the [`cadmus-core` runtime cost model](../core/AGENTS.md) for where blocking
work belongs.

## Event loop

The main loop owns the hub channel and the view [`Bus`](../core/src/view/mod.rs);
events are dispatched to the device lifecycle first, then to the view tree. See
[`src/app.rs`](src/app.rs).

## Shutdown

On exit, `stop_producers_and_drain` stops every producer, then drains only
**shutdown-related** events:

- the direct exit intents — `Select(Reboot)`, `Select(Restart)`,
  `Select(PowerOff)`, `Select(Quit)`, and `Quit`;
- `Event::CheckBattery`, which can return a low-battery `Exit(PowerOff)` and
  records battery state before teardown.

Every other queued event is dropped. Replaying an arbitrary queued device event
during teardown would run transitions such as `PrepareShare` (which closes the
database and enables mass storage) after the process has already begun exiting.

View-owned [`Job`s](../core/src/runtime.rs) are cancelled while each view still
holds its handle, then moved out and joined within `SHUTDOWN_DEADLINE`.

See the background task stop/join policy in
[`../core/src/task/AGENTS.md`](../core/src/task/AGENTS.md).
