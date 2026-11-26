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
4. An owned Classic job first runs `prepare_suspend`: `state-extended=1`, an
   async 2 s settle, then `sync()` on the blocking pool. It does not write
   `state=mem`. After a cancel check, the job takes the Full gate token and
   rechecks Full; Full registration takes the same gate, so a Full acquire
   arriving while the gate is held waits for it.
5. After the wake-lock pause, `suspend` submits only `state=mem`; that sysfs
   write blocks on the runtime's blocking pool until the kernel resumes. The
   job and gate token stay owned through wake, peripheral cleanup, and
   wake-lock restoration even if the event caller stops waiting. A Full
   acquire that waited on the gate aborts the cycle only after that cleanup.
   `resume` clears `state-extended=0` and re-inits model-specific touch;
   AutoPowerOff / Calendar RTC; WakeDebounce re-sleep until user cancels

## Soft-suspend deep idle (autosleep armed)

When soft suspend is armed, same triggers use
<a href="/api/cadmus_core/device/suspend/cycle/enum.SuspendKind.html#variant.DeepIdle">`DeepIdle`</a>:

1. `start_cycle` acquires the `deep-idle` cycle lease
2. PrepareSuspend teardown runs with zero delay; there is no Suspend RTC
3. Force autosleep **`mem`**, `arm_deep_idle` (`state-extended=1`), release the
   shared `cadmus` wake lock while keeping every SoftSuspend lease tracked, then
   drop the cycle lease
4. Poll boottime−monotonic until wake or timeout; wake restores `cadmus` while
   any SoftSuspend holder remains; re-arm while soft suspend
   can re-arm, otherwise `finish_cycle`. DeepIdle stops after three
   consecutive timed-out attempts; a detected wake resets the count, so the
   WakeDebounce re-entry starts fresh
5. Past the cap, fall back to `Classic` for the rest of the cycle: autosleep
   is disarmed (`Off`) so the blocking `state=mem` write is not refused by the
   armed-mode guard, and `finish_cycle` restores the mode; a refused `Classic`
   attempt retries via WakeDebounce
6. Wake: disarm, restore autosleep mode; WakeDebounce / CalendarUpdate re-enter
   with the cycle kind (`DeepIdle`, or `Classic` after the fallback)

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
| `KoboPowerManager::suspend`          | Classic `state=mem` write only                        |

<!-- i18n:skip-end -->
