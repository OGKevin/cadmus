<!-- i18n:skip-start -->

# 0001: Async device I/O with serial event handling

| Status   | Date       |
| -------- | ---------- |
| accepted | 2026-11-28 |

Suspend, wake and their hardware I/O (sysfs writes, ioctls, RTC calls, settings
files) used to run as blocking calls on the main thread. Input froze while they
ran, and moving them to background threads by hand left sequences that could be
cut off half-way. We decided:

1. Device traits are async. Hardware traits
   (<a href="/api/cadmus_core/device/power/trait.PowerManager.html">`PowerManager`</a>,
   <a href="/api/cadmus_core/device/rtc/trait.Rtc.html">`Rtc`</a>) return
   `impl Future + Send`. Traits used as `dyn`
   (<a href="/api/cadmus_core/device/battery/trait.Battery.html">`Battery`</a>,
   <a href="/api/cadmus_core/frontlight/trait.Frontlight.html">`Frontlight`</a>)
   use [`#[async_trait]`](https://docs.rs/async-trait). View and settings traits
   (<a href="/api/cadmus_core/view/trait.View.html">`View`</a>,
   <a href="/api/cadmus_core/view/settings_editor/kinds/trait.SettingKind.html">`SettingKind`</a>,
   <a href="/api/cadmus_core/framebuffer/trait.Framebuffer.html">`Framebuffer`</a>)
   use `#[async_trait(?Send)]` because they run only on the main thread.
   Blocking calls with no async API run in
   [`spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html).
2. Hardware sequences that must not stop half-way run in tasks that own their
   state, so dropping the caller cannot cancel them. Examples are
   `alarm_transaction` for RTC alarm changes and the Classic commit for
   prepare, `state=mem` and resume.
3. The main loop handles one event at a time and always awaits the handler to
   completion. No [`select!`](https://docs.rs/tokio/latest/tokio/macro.select.html),
   `timeout` or abort may wrap a handler. `select!` only picks the next message.

## Why the main loop never races a handler

A future dropped by `select!` stops at whichever `.await` it was parked on.
Handlers change `AppContext` across several awaits: for example, the kernel wake
lock is already released but the suspend cycle has not stored its suppression
epoch yet, or the frontlight is off but no cycle exists to turn it back on.
Handling one event at a time also gives `AppContext` a single writer, so it needs
no locks, and keeps hub order as the order things happen. The Wi-Fi reconcile
generation check relies on that order.

## Consequences

- A long handler holds the loop. The Classic commit awaits for the whole kernel
  sleep, which is fine because the device is asleep.
- Every new handler keeps rule 3. Work that must outlive its caller goes into an
  owned task (rule 2), not into a raced future.
- Cancellation-safety is reviewed per owned task, not per handler.

## Considered options

- **Make every handler cancellation-safe.** Rejected: every await in every
  handler would need a rollback, and each new handler would carry that burden.
- **Keep sync traits and call `spawn_blocking` at each call site.** Rejected: a
  missed call site blocks the main thread again, and nothing in the type system
  catches it.

<!-- i18n:skip-end -->
