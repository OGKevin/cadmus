<!-- i18n:skip-start -->

# Kobo suspend

Kobo-specific wiring for the shared
[suspend orchestrator](../orchestrator.md). Soft-suspend lease / session details
live in [Soft Suspend](../soft-suspend.md). Research notes are in
[investigation #361](../../../../investigations/kobo/issue-361-autosleep-wake-lock.md).

Kobo `DeviceLifecycle` is a thin facade: PrepareSuspend / Suspend /
PollDeepIdleWait / suspend-related RTC alarms forward to
`crate::device::suspend`. Kind (`Classic` \| `DeepIdle`) and phase live on
`AppContext::suspend`.

## Classic hard suspend (soft suspend Off)

When `AutosleepMode` is `Off`, Auto Suspend / power button / sleep cover use
the classic path via the orchestrator:

1. RTC `AlarmType::AutoSuspend` fires → `start_cycle` (`Classic`)
2. Intermission UI; wait prepare delay (3s) then PrepareSuspend teardown
3. Wait suspend delay (15s) then `Event::Suspend`
4. <a href="/api/cadmus_core/device/kobo/power/struct.KoboPowerManager.html#method.suspend">`KoboPowerManager::suspend`</a>:
   write `1` to `/sys/power/state-extended`, sync, write `mem` to
   `/sys/power/state`
5. On wake: `state-extended=0` (+ model-specific touch re-init); AutoPowerOff /
   Calendar RTC handling; re-sleep until the user cancels

## Soft-suspend deep idle (mode armed)

When soft suspend is armed, the same triggers use `DeepIdle`:

1. Acquire a named `deep-idle` cycle lease through the orchestrator
2. PrepareSuspend runs immediately, then deep-idle wait (no Suspend RTC)
3. Force session autosleep to **`mem`**, arm `state-extended=1`, drop the lease
4. Poll until wake (boottime−monotonic) or timeout → retry / finish
5. On wake: `state-extended=0`, restore autosleep mode; WakeDebounce /
   CalendarUpdate re-enter with the same `DeepIdle` kind

## `state-extended` is Kobo/NiX, not mainline

[`/sys/power/state-extended`](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-power)
does **not** appear in mainline `sysfs-power`. On Kobo it is a vendor kernel
patch: writing `1` runs platform hooks (e.g. Neonode touch off); writing `0`
reverses them. Writing `1` alone does **not** suspend the device.

Upstream autosleep (`/sys/power/autosleep` + wake locks) is what actually
sleeps when soft suspend is armed.

## Related APIs

| Piece                                                                                                                                                                                                                                   | Role                                       |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| <a href="/api/cadmus_core/device/suspend/index.html">`device::suspend`</a> orchestrator                                                                                                                                                 | Kind, phase, begin → finish                |
| <a href="/api/cadmus_core/device/rtc/enum.AlarmType.html#variant.AutoSuspend">`AlarmType::AutoSuspend`</a> / <a href="/api/cadmus_core/device/rtc/struct.AlarmManager.html">`AlarmManager`</a>                                          | Wall-clock idle deadline                   |
| <a href="/api/cadmus_core/device/soft_suspend/struct.SoftSuspendSession.html">`SoftSuspendSession`</a>                                                                                                                                  | Autosleep mode + `cadmus` wake lock leases |
| <a href="/api/cadmus_core/device/power/trait.PowerManager.html#method.arm_deep_idle">`PowerManager::arm_deep_idle`</a> / <a href="/api/cadmus_core/device/power/trait.PowerManager.html#method.disarm_deep_idle">`disarm_deep_idle`</a> | Kobo `state-extended`                      |
| <a href="/api/cadmus_core/device/kobo/power/struct.KoboPowerManager.html#method.suspend">`KoboPowerManager::suspend`</a>                                                                                                                | Classic `state-extended` + `state=mem`     |

<!-- i18n:skip-end -->
