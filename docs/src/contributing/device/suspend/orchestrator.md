<!-- i18n:skip-start -->

# Suspend orchestrator

**Explicit suspend**: intermission UI, prepare teardown, then kernel sleep.
Shared code in
<a href="/api/cadmus_core/device/suspend/index.html">`device::suspend`</a>
(`crates/core/src/device/suspend/`). This is separate from opportunistic
[Soft suspend](soft-suspend.md) and from
[Kobo platform hooks](kobo/suspend.md); the emulator does not drive it.

Callers that mean "go to sleep" (power button, Auto Suspend RTC, sleep cover)
call
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>,
**not**
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.enter_sleep.html">`enter_sleep`</a>.
`Event::Suspend` and `AlarmType::Suspend` mean **enter sleep now** after
prepare, not "start a new cycle". Soft suspend leases alone do not gate
`start_cycle`; only
[Full](inhibitor.md) does, while [Sleep](inhibitor.md) defers just the Auto
Suspend path.

## Kind and phase

The kind is fixed once at `start_cycle` from Soft suspend armedness: `DeepIdle`
when armed, otherwise `Classic`. If the DeepIdle cycle lease cannot be acquired,
`start_cycle` switches the cycle to `Classic` before scheduling prepare. Do not
re-pick the kind mid-cycle from `is_armed()`; WakeDebounce and CalendarUpdate
re-entry keep the cycle's kind.

`finish_cycle` is the only teardown path, and it restores the wake lock only
when the exiting sleep leg still owns suppression, so one leg never restores
twice. A cancel that lands while only a prepare task is pending still restores
the frontlight and intended Wi-Fi state.

## Sleep backends

`PowerManager` is async end to end, but its sysfs and `sync()` work runs on the
runtime's capped blocking pool, not on the reactor workers.

**Classic** runs one owned commit. Cancellation is cooperative only before the
gate token is acquired; after that, dropping the task or its waiter cannot
cancel the write, wake-lock restoration, or peripheral cleanup, and cycle
finish/shutdown drain the commit before further hardware work. A
preparation failure, unlock failure, or `mem` failure is **not** a wake: only a
successful `mem` runs post-wake alarm classification. A Classic suspend with no
cycle is refused while Soft suspend is armed, and Auto Suspend is rescheduled
instead.

**DeepIdle** sleeps through autosleep rather than the blocking `state=mem`
write. The pre-sleep alarm state is captured before autosleep is armed; the
boottime and monotonic anchors are captured after arming and `sync()`
immediately before the pause, because they drive wake detection, while only the
wall-clock `sleep_started_at` is used for post-wake RTC alarm claims. The
suppression epoch ensures a pause ticket from an earlier leg is never current,
so a Full inhibition lease that arrives and leaves between pause and the next poll still
aborts the cycle instead of producing a timeout or a synthetic wake.

Event dispatch stays serial: a cover-open queued during Classic preparation
waits for the current handler, not an interrupt-driven cancel. Last-moment Full
checks still happen at the DeepIdle pause and the Classic commit boundary; a
Full acquired mid-cycle aborts the cycle before sleep, or after Classic cleanup
once the `mem` write is already committed — see
[Deferred Explicit suspend](inhibitor.md#deferred-explicit-suspend).

## Deep-idle wake detect

Production compares `CLOCK_BOOTTIME` against `CLOCK_MONOTONIC`, so an NTP step
does not look like a wake; tests inject
<a href="/api/cadmus_core/device/suspend/wake/enum.PollResult.html">`PollResult`</a>
instead.

On timeout the wait re-arms while Soft suspend can re-arm, otherwise
`finish_cycle`. After three consecutive timed-out attempts the cycle falls back
to `Classic` and disarms autosleep for the rest of the cycle, because the kernel
rejects `state=mem` while autosleep is armed, which would turn every attempt
into the same guard error; `finish_cycle` restores the stashed mode. The cap
counts consecutive timeouts, so a detected wake resets it and a cycle can make
more than three DeepIdle attempts when wakes interleave. The first timeout of a
failing cycle logs the lease holders and kernel wake state once, because
autosleep refusals carry no errno.

## Wake-source telemetry

On Linux, the DeepIdle wait snapshots the pre-sleep hardware and earliest
logical alarm and the IRQ counters, and each detected wake emits one
`Primary deep-idle wake` record from
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.log_wake_source.html">`log_wake_source`</a>.
What the fields cannot say matters more than what they contain:

- `wake_irq` is read inside the wake path because the kernel clears it on the
  next suspend. A missing value means "not reported", not "no hardware wake".
- `wake_irq_name` names the owning `/proc/interrupts` row. A parent controller
  such as the PMIC does not identify its child, so compare per-IRQ counters
  before naming a source.
- `unmanaged_alarm_candidate` only reports that the hardware slot and the
  earliest future logical alarm disagree. The kernel records no writer for the
  slot, so this is a candidate, never proof of who wrote it.
- IRQ deltas are evidence of activity between samples, not proof of wake
  causation; they can also include activity before sleep or after wake. A
  `partial` status explains which rows could not be compared.

<!-- i18n:skip-end -->
