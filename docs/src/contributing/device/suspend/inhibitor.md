<!-- i18n:skip-start -->

# Inhibitor

Caller-facing API for acquiring inhibitor guards. Acquires go through
<a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html#method.acquire">`Inhibitor::acquire`</a>
with `Kind::SoftSuspend`, `Kind::Sleep`, or `Kind::Full`.

Implementation: `crates/core/src/device/inhibitor/`.

Overview of how this relates to
[explicit suspend](orchestrator.md) vs
[soft suspend](soft-suspend.md):
[Suspend index](index.md#blocking-sleep-with-the-inhibitor).

## Kinds

| Kind                                                                                             | Wake lock                      | Blocks [explicit suspend](orchestrator.md) on Kobo | Blocks opportunistic [soft suspend](soft-suspend.md) | Blocks Kobo user exits |
| ------------------------------------------------------------------------------------------------ | ------------------------------ | -------------------------------------------------- | ---------------------------------------------------- | ---------------------- |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.SoftSuspend">`SoftSuspend`</a> | Yes (Linux)                    | **No**                                             | **Yes** (while guard held)                           | No                     |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Sleep">`Sleep`</a>             | Yes (nested `"sleep-inhibit"`) | Auto Suspend path only                             | **Yes**                                              | No                     |
| <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Full">`Full`</a>               | Yes (nested `"full-inhibit"`)  | **Yes**                                            | **Yes**                                              | Yes                    |

## Keep the device awake

Hold the returned
<a href="/api/cadmus_core/device/inhibitor/struct.InhibitorGuard.html">`InhibitorGuard`</a>
for the lifetime of the work or UI. Dropping it releases the lease.

- **`Kind::SoftSuspend`** keeps the Linux wake lock held while work runs.
  Examples: input handling, import tasks, and main-loop work.
  `WifiSession` holds `SoftSuspendName::Wifi` internally while it has Wi-Fi
  leases such as `"ota-download"`. Explicit Auto Suspend / power-button sleep
  can still run.
- **`Kind::Sleep`** blocks opportunistic autosleep and Auto Suspend without
  blocking user exits or explicit suspend (menu Suspend, power button, cover).
  Live AutoPowerOff is not blocked. No battery gate, no status LED, last drop
  does not post `FullInhibitCleared`. cadmus-test holds this kind while the top
  menu (`ViewId::MainMenu`) is open:

  ```rust
  let _guard = inhibitor.acquire(Kind::Sleep, "main-menu")?;
  ```

- **`Kind::Full`** is used for critical sections such as OTA install (`"ota"`).
  On Kobo it blocks
  <a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>,
  menu power-off/restart/reboot/quit/switch-install and long-press power-off.
  A live `RtcAlarmFired(AutoPowerOff)` event is ignored while Full is active.
  Battery-monitor safety power-off is **not** blocked.

Sleep and Full holders use free-form names (`"main-menu"`, `"ota"`). SoftSuspend
uses
<a href="/api/cadmus_core/device/inhibitor/enum.SoftSuspendName.html">`SoftSuspendName`</a>
where a standard name exists.

### Full battery gate

`Kind::Full` acquire reads shared
<a href="/api/cadmus_core/device/battery/trait.Battery.html">`Battery`</a>
capacity and fails with
<a href="/api/cadmus_core/device/inhibitor/enum.InhibitorError.html#variant.BatteryTooLow">`BatteryTooLow`</a>
when capacity cannot be read or the first reported cell is below 20% (the Kobo
`KoboRoot` install floor). `Kind::SoftSuspend` and `Kind::Sleep` are never
battery-gated.

## Deferred explicit suspend

While any Full holder is active,
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>
sets `Context::deferred_suspend` and returns without a cycle.

The Auto Suspend path
(<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle_from_auto.html">`start_cycle_from_auto`</a>)
also defers while Sleep is held. Explicit `start_cycle` (cover, power button,
menu Suspend) still runs during Sleep.

When the last Full holder drops, Kobo posts
<a href="/api/cadmus_core/view/enum.Event.html#variant.FullInhibitCleared">`Event::FullInhibitCleared`</a>;
the orchestrator flushes on the main loop. Last Sleep drop does **not** flush.
The next user gesture calls
<a href="/api/cadmus_core/device/fn.reschedule_auto_suspend_alarm.html">`reschedule_auto_suspend_alarm`</a>,
which clears deferred intent without starting a cycle.

Calling `reschedule_auto_suspend_alarm` also clears deferred intent without
flushing on other user-activity paths.

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
