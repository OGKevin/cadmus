<!-- i18n:skip-start -->

# Inhibitor

Caller-facing API for acquiring inhibitor guards. Acquires are async: call
<a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html#method.acquire">`Inhibitor::acquire`</a>
with `Kind::SoftSuspend`, `Kind::Sleep`, or `Kind::Full` and `.await?` it.

Implementation: `crates/core/src/device/inhibitor/`.

Overview of how this relates to
[explicit suspend](orchestrator.md) vs
[soft suspend](soft-suspend.md):
[Suspend index](index.md#blocking-sleep-with-the-inhibitor).

## Kinds

| Kind                                                                                             | Wake lock                                   | Blocks [explicit suspend](orchestrator.md) on Kobo | Blocks opportunistic [soft suspend](soft-suspend.md) | Blocks Kobo user exits |
| ------------------------------------------------------------------------------------------------ | ------------------------------------------- | -------------------------------------------------- | ---------------------------------------------------- | ---------------------- |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.SoftSuspend">`SoftSuspend`</a> | Yes (Linux; suppressed during kernel sleep) | **No**                                             | **Yes** (while guard held)                           | No                     |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Sleep">`Sleep`</a>             | Yes (nested `"sleep-inhibit"`)              | **No**                                             | **Yes**                                              | No                     |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Full">`Full`</a>               | Yes (nested `"full-inhibit"`)               | **Yes**                                            | **Yes**                                              | Yes                    |

## Keep the device awake

Hold the returned
<a href="/api/cadmus_core/device/inhibitor/struct.InhibitorGuard.html">`InhibitorGuard`</a>
for the lifetime of the work or UI. Dropping it releases the lease.

- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.SoftSuspend">`Kind::SoftSuspend`</a>
  keeps the Linux wake lock held while work runs. Examples: input handling,
  import tasks, and main-loop work.
  <a href="/api/cadmus_core/device/wifi/struct.WifiSession.html">`WifiSession`</a>
  holds
  <a href="/api/cadmus_core/device/inhibitor/enum.SoftSuspendName.html#variant.Wifi">`SoftSuspendName::Wifi`</a>
  internally while it has Wi-Fi leases such as `"ota-download"`. Explicit Auto
  Suspend / power-button sleep can still run: at the kernel-sleep boundary the
  shared `cadmus` lock is released immediately and re-acquired on wake while
  any holder remains.
- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Sleep">`Kind::Sleep`</a>
  blocks opportunistic autosleep and Auto Suspend without blocking user exits
  or explicit suspend (menu Suspend, power button, cover). The nested
  `"sleep-inhibit"` lease stays tracked through preparation and the explicit
  cycle. At the kernel-sleep boundary it is suppressed with every other
  SoftSuspend holder; kernel wake and `finish_cycle` re-acquire the shared
  `cadmus` lock if holders remain (top menu still open). Live AutoPowerOff is
  not blocked. No status LED. Acquire shares the battery gate described below.
  cadmus-test holds this kind while the top menu
  (<a href="/api/cadmus_core/view/enum.ViewId.html#variant.MainMenu">`ViewId::MainMenu`</a>)
  is open:

  ```rust
  let _guard = inhibitor.acquire(Kind::Sleep, "main-menu").await?;
  ```

- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Full">`Kind::Full`</a>
  is used for critical sections such as OTA install (`"ota"`). On Kobo it blocks
  <a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>,
  menu power-off/restart/reboot/quit/switch-install and long-press power-off.
  A live `RtcAlarmFired(AutoPowerOff)` event is ignored while Full is active.
  Battery-monitor safety power-off is **not** blocked. Internal `Event::Exit`
  events are not user intent and are never refused. A live Full holder also
  blocks <a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html">`Inhibitor`</a>
  suppression of the shared `cadmus` lock; if Full is acquired during an
  active suppression window, the nested `"full-inhibit"` lease immediately
  re-locks `cadmus` before the next suspend poll runs.

Sleep and Full holders use free-form names (`"main-menu"`, `"ota"`). SoftSuspend
uses
<a href="/api/cadmus_core/device/inhibitor/enum.SoftSuspendName.html">`SoftSuspendName`</a>
where a standard name exists.

### Battery gate

`Kind::Sleep` and `Kind::Full` acquire read shared
<a href="/api/cadmus_core/device/battery/trait.Battery.html">`Battery`</a>
capacity and fail with
<a href="/api/cadmus_core/device/inhibitor/enum.InhibitorError.html#variant.BatteryTooLow">`BatteryTooLow`</a>
when capacity cannot be read or the first reported cell is below 20% (the Kobo
`KoboRoot` install floor). One threshold serves both kinds. `Kind::SoftSuspend`
is never battery-gated.

### Wake-lock write failures

Wake-lock reconciliation uses one writer: demand is recomputed after every
completed sysfs write, and only that write changes `held`. Pause reports
`Paused(epoch)`, `BlockedByFull`, or `UnlockFailed`; resume reports `Restored`,
`Unheld`, `RestoreFailed`, or `WorkerFailed` (the control worker stopped or
closed its request channel). A missing live sysfs node is an error, not a NoOp
success. A Full acquire with unconfirmed kernel protection fails rather than
letting critical work run unprotected.

A failed restore queues at most three backend retries (100, 200, and 400 ms),
with demand recomputed each time, even if the only holder never reacquires.
Obsolete work is cancelled after lease loss, fresh suppression, convergence,
or shutdown. Exhaustion retains the mismatch for a later explicit resume or
acquisition. Cycle finish and a Classic commit stop and drain any in-flight
suppression leg before starting another one.

`acquire` is the async acquisition path for main-loop, input, cycle, and
shutdown leases. Lease registration is cheap; hardware confirmation is awaited
through dedicated control workers. Lease drops update intent and notify the
reconcile worker, including zero grace, so hardware I/O does not block event
dispatch. At application shutdown, `Inhibitor::shutdown` requests final release
and awaits the control worker's result; dropping the backend does not perform a
blocking write on the dropping thread.

## Deferred explicit suspend

While any Full holder is active,
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>
queues `Context::deferred_suspend` as **Explicit** and returns without a cycle.

The Auto Suspend path
(<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle_from_auto.html">`start_cycle_from_auto`</a>)
queues **Auto** while Full or Sleep is held. Explicit `start_cycle` (cover,
power button, menu Suspend) still runs during Sleep. The shared `cadmus` lock
is suppressed at the kernel-sleep boundary and re-acquired on wake if any
SoftSuspend holder remains, including the nested `"sleep-inhibit"` lease while
Sleep holders remain.

If a worker acquires Full after a cycle starts, the orchestrator aborts the
cycle before kernel sleep during preparation or a DeepIdle wait. A Classic
commit holds the Full suspend gate from its final Full recheck through `mem`,
wake, peripheral cleanup, and wake-lock restoration, and `Kind::Full` acquire
takes the same gate. A Full acquire during a held Classic gate therefore waits
for it, cannot interrupt the write, and the abort happens after wake and
cleanup if Full is still held; a Full holder acquired before the gate is seen
aborts before `mem`.

The abort preserves the cycle's current origin and any newer queued request:
Auto remains Auto, while Explicit wins over Auto. If Full has already cleared,
the intent is flushed immediately; otherwise it waits for
`FullInhibitCleared`.

When the last Full holder drops, Kobo posts
<a href="/api/cadmus_core/view/enum.Event.html#variant.InhibitCleared">`Event::InhibitCleared(Kind::Full)`</a>;
the orchestrator flushes on the main loop. Auto origin goes through
`start_cycle_from_auto`; Explicit origin runs `start_cycle` even if Sleep is
held.

When the last Sleep holder drops, Kobo posts `InhibitCleared(Kind::Sleep)`. The
orchestrator flushes a waiting **Auto** through `start_cycle_from_auto`, which
re-defers while a Full lease or another Sleep lease is held; a waiting
**Explicit** suspend stays queued for the Full release. A release during a
suspend cycle starts nothing.

A touch calls
<a href="/api/cadmus_core/device/fn.reschedule_auto_suspend_alarm.html">`reschedule_auto_suspend_alarm`</a>,
which clears deferred intent without starting a cycle.

**OTA success** posts
<a href="/api/cadmus_core/view/enum.Event.html#variant.ClearDeferredSuspend">`Event::ClearDeferredSuspend`</a>
before releasing `"ota"` so deferred intent does not race the reboot. Clearing
deferred suspend does **not** re-arm
<a href="/api/cadmus_core/device/rtc/enum.AlarmType.html#variant.AutoSuspend">`AutoSuspend`</a>;
that requires a later call to `reschedule_auto_suspend_alarm`.

## Release before reboot

Holders that reboot must drop Full **first**, then send the reboot event. OTA:
`ClearDeferredSuspend`, then the worker's `"ota"`
<a href="/api/cadmus_core/device/inhibitor/struct.InhibitorGuard.html">`InhibitorGuard`</a>
drops, then the reboot delay.

## Status LED arbiter

<a href="/api/cadmus_core/device/leds/struct.StatusLed.html">`StatusLed`</a>
multiplexes one hardware LED. Higher
<a href="/api/cadmus_core/device/leds/enum.LedPriority.html">`LedPriority`</a>
wins. Sleep inhibit does not drive the LED.

| Client        | Name            | Priority                            | Pattern                              |
| ------------- | --------------- | ----------------------------------- | ------------------------------------ |
| Full inhibit  | `full-inhibit`  | `LedPriority::FullInhibit` (higher) | Pulse 1600 ms on / 200 ms off        |
| Soft indicate | `soft-indicate` | `LedPriority::SoftIndicate` (lower) | Solid on (setting + autosleep armed) |

User-facing explanation: [Soft Suspend § Status LED](../../../soft-suspend.md#status-led-indicator).

## OTA

`run_ota_download`: WiFi lease → `Kind::Full, "ota"` as a local
<a href="/api/cadmus_core/device/inhibitor/struct.InhibitorGuard.html">`InhibitorGuard`</a>
→ download/install. The guard drops when the worker returns (success, cancel,
failure, re-auth, panic). Success sends `ClearDeferredSuspend` before that
drop, then schedules reboot.

<!-- i18n:skip-end -->
