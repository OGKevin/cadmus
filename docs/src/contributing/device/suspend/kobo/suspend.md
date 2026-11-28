<!-- i18n:skip-start -->

# Kobo suspend

Kobo-specific **platform** wiring for the shared
[orchestrator](../orchestrator.md). Lifecycle forwards suspend events to
<a href="/api/cadmus_core/device/suspend/index.html">`device::suspend`</a>;
kind and phase live on
<a href="/api/cadmus_core/context/struct.Context.html#structfield.suspend">`Context::suspend`</a>.
Conceptual overview: [Suspend index](../index.md).

## Classic hard suspend (autosleep Off)

With
<a href="/api/cadmus_core/device/soft_suspend/mode/enum.AutosleepMode.html#variant.Off">`AutosleepMode::Off`</a>,
Auto Suspend, the power button, and the sleep cover use
<a href="/api/cadmus_core/device/suspend/cycle/enum.SuspendKind.html#variant.Classic">`Classic`</a>.
After prepare teardown a Suspend RTC reaches `enter_sleep`, which runs an owned
commit. `prepare_suspend` arms `state-extended=1`, settles, and flushes buffers;
the commit then takes the Full gate, pauses the wake lock, and writes only
`state=mem`. That write blocks on the blocking pool until the kernel resumes.

The commit and gate stay owned through wake, peripheral cleanup, and wake-lock
restoration even if the event caller stops waiting, so a Full request that
arrives after the last check aborts the cycle only after that cleanup. Resume
clears `state-extended=0` and re-initialises the touch controller on the models
that need it; AutoPowerOff and Calendar RTCs are then checked, and WakeDebounce
may re-enter sleep with the same cycle until User activity cancels it. Every
wake is classified as a User or Unattended wake with its cause, and an RTC that
fires before its alarm is due is reported and re-armed — see
[Wake-source telemetry](../orchestrator.md#wake-source-telemetry).

## Soft-suspend deep idle (autosleep armed)

When Soft suspend is armed the same triggers use
<a href="/api/cadmus_core/device/suspend/cycle/enum.SuspendKind.html#variant.DeepIdle">`DeepIdle`</a>.
`start_cycle` acquires a `deep-idle` Soft suspend lease, prepare runs with no
delay, and there is no Suspend RTC. The wait arms autosleep `mem` and kernel
deep idle, releases the shared `cadmus` wake lock while keeping external Soft
suspend leases tracked, then drops the cycle's own `deep-idle` lease. That drop
keeps the cycle from counting as a holder when wake decides whether to re-lock
`cadmus`.

The wait polls boottime−monotonic for wake or timeout; wake re-locks `cadmus`
while any Soft suspend holder remains, and the three-timeout fallback is
described in [Deep-idle wake detect](../orchestrator.md#deep-idle-wake-detect).

If the `deep-idle` lease cannot be acquired, `start_cycle` changes the cycle to
`Classic` before scheduling prepare.

Opportunistic naps between events still use Soft suspend leases; DeepIdle is
the **explicit** path after Auto Suspend fires.

## User activity

The decoder in <a href="/api/cadmus_core/input/index.html">`input`</a> decides
what counts as User activity before the lifecycle sees it: a finger down or
move, a decoded button press, opening the sleep cover, and a gyroscope rotation
to another orientation while the device is awake. An empty touchscreen frame
reports nothing (the controller sends one per resume), and neither does
plugging in a cable. A host plug still cancels a pending suspend and may start
USB sharing; it is just not User activity.

## `state-extended`

Kobo power code writes `1` when arming sleep and `0` when disarming it. Writing
`1` does not itself enter `mem`: Classic writes `state=mem` separately, and
DeepIdle relies on autosleep after dropping its cycle lease. Disarming also
restores the touch controller.

<!-- i18n:skip-end -->
