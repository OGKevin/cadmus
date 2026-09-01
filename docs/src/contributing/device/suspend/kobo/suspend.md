<!-- i18n:skip-start -->

# Kobo suspend

Kobo-specific **platform** wiring for the shared
[orchestrator](../orchestrator.md). Lifecycle forwards suspend events to
<a href="/api/cadmus_core/device/suspend/index.html">`device::suspend`</a>;
kind and phase live on
<a href="/api/cadmus_core/context/struct.Context.html#structfield.suspend">`Context::suspend`</a>.

Conceptual overview: [Suspend index](../index.md).

## Classic hard suspend (autosleep Off)

When
<a href="/api/cadmus_core/device/soft_suspend/mode/enum.AutosleepMode.html#variant.Off">`AutosleepMode::Off`</a>,
Auto Suspend / power button / sleep cover use
<a href="/api/cadmus_core/device/suspend/cycle/enum.SuspendKind.html#variant.Classic">`Classic`</a>:

1. `AlarmType::AutoSuspend` → `start_cycle` (`Classic`)
2. Intermission; 3 s prepare delay → PrepareSuspend teardown
3. 15 s Suspend RTC → `RtcAlarmFired(Suspend)` → `enter_sleep`
4. `KoboPowerManager::suspend`: `state-extended=1`, wait 2 s, `sync`,
   `state=mem`
5. Wake: `state-extended=0` and model-specific touch re-init; AutoPowerOff /
   Calendar RTC; WakeDebounce re-sleep until user cancels

## Soft-suspend deep idle (autosleep armed)

When soft suspend is armed, same triggers use
<a href="/api/cadmus_core/device/suspend/cycle/enum.SuspendKind.html#variant.DeepIdle">`DeepIdle`</a>:

1. `start_cycle` acquires the `deep-idle` cycle lease
2. PrepareSuspend teardown runs with zero delay; there is no Suspend RTC
3. Force autosleep **`mem`**, `arm_deep_idle` (`state-extended=1`), drop lease
4. Poll boottime−monotonic until wake or timeout → retry / `finish_cycle`
5. Wake: disarm, restore autosleep mode; WakeDebounce / CalendarUpdate keep
   `DeepIdle` kind

If the `deep-idle` lease cannot be acquired, `start_cycle` changes the cycle to
Classic before scheduling PrepareSuspend.

Opportunistic naps between events still use
[soft suspend](../soft-suspend.md) leases — DeepIdle is the **explicit** path
after Auto Suspend fires.

## `state-extended`

Kobo power code writes `1` to `/sys/power/state-extended` when arming sleep and
`0` when disarming it. Writing `1` does not itself call the blocking
`state=mem` path. Classic writes `state=mem` separately; DeepIdle relies on
autosleep after dropping its cycle lease.

## Related APIs

| Piece                                | Role                                                  |
| ------------------------------------ | ----------------------------------------------------- |
| `device::suspend`                    | Cycle orchestration                                   |
| `AlarmManager` / `AutoSuspend`       | Wall-clock idle deadline                              |
| `Inhibitor`                          | Wake locks + Full gate — [Inhibitor](../inhibitor.md) |
| `arm_deep_idle` / `disarm_deep_idle` | DeepIdle `state-extended`                             |
| `KoboPowerManager::suspend`          | Classic sleep                                         |

<!-- i18n:skip-end -->
