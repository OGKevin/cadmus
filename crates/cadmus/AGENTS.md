# Cadmus binary (`cadmus`)

Entry goes through [`cadmus_core::runtime::enter`](../core/src/runtime.rs),
which builds the capped multi-thread runtime and bounds shutdown. Do not add
`#[tokio::main]`: the app future runs on the **main thread** so exit can apply
`SHUTDOWN_DEADLINE` after it returns. Blocking the main thread freezes the UI;
see the [`cadmus-core` runtime cost model](../core/AGENTS.md) for where blocking
work belongs.

## Event loop

The main loop owns the hub channel and the view [`Bus`](../core/src/view/mod.rs).
Events go to background tasks first, then the device lifecycle, then the view
tree. See [`src/app.rs`](src/app.rs).

## Shutdown

After `background_tasks.stop_all().await`, `stop_producers_and_drain` clears
device tasks (`tasks.clear()`, which cancels their `Job`s), cancels and joins
view jobs within `SHUTDOWN_DEADLINE`, then drains only **shutdown-related**
events:

- the direct exit intents — `Select(Reboot)`, `Select(Restart)`,
  `Select(PowerOff)`, `Select(Quit)`, and `Quit`;
- `Event::CheckBattery`, which can return a low-battery `Exit(PowerOff)` and
  records battery state before teardown.

Every other queued event is dropped. Replaying an arbitrary queued device event
during teardown would run transitions such as `PrepareShare` (which closes the
database and enables mass storage) after the process has already begun exiting.

Clearing device tasks only requests cancellation; they are not joined, so a
task that has not yet observed its token can still queue an event during the
drain. View jobs are cancelled and joined. Input stays live. Events from either
source are dropped unless they are shutdown-related.

See the background task stop/join policy in
[`../core/src/task/AGENTS.md`](../core/src/task/AGENTS.md).
