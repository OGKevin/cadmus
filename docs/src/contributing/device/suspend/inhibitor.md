<!-- i18n:skip-start -->

# Inhibitor

Named leases that tell Cadmus, and on Linux the kernel, that work must not be
interrupted. Acquire one with
<a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html#method.acquire">`Inhibitor::acquire`</a>
and hold the guard for the lifetime of the work; dropping it releases the
lease. Implementation: `crates/core/src/device/inhibitor/`.

## Kinds

- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.SoftSuspend">`Kind::SoftSuspend`</a>
  holds the shared `cadmus` wake lock while work runs. It blocks neither
  Explicit suspend nor user exits and is never battery-gated.
- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Sleep">`Kind::Sleep`</a>
  additionally defers the Auto Suspend path. Explicit suspend (menu, power
  button, cover) still runs, and user exits are not blocked. No status LED.
- <a href="/api/cadmus_core/device/inhibitor/enum.Kind.html#variant.Full">`Kind::Full`</a>
  additionally blocks starting an Explicit suspend and ignores user
  power-off/restart/reboot/quit/switch-install and long-press power-off. A live
  `AutoPowerOff` event is ignored, but battery-monitor safety power-off is not;
  internal `Event::Exit` values are not user intent and are never refused.

Sleep and Full each register one nested Soft suspend lease (`"sleep-inhibit"`,
`"full-inhibit"`) so the kernel lock stays held for the whole window even when
no other holder exists. Named leases do not reach the kernel separately: all
Soft suspend holders collapse onto the single `cadmus` lock.

### Battery gate

Sleep and Full read the shared
<a href="/api/cadmus_core/device/battery/trait.Battery.html">`Battery`</a>
before registering and fail with
<a href="/api/cadmus_core/device/inhibitor/enum.InhibitorError.html#variant.BatteryTooLow">`BatteryTooLow`</a>
when capacity is unreadable or the first cell is below 20%. The floor matches
the Kobo `KoboRoot` install requirement rather than the Auto Power Off setting,
because holding a nearly empty device awake has the same failure mode.
Soft suspend is never gated.

### Wake-lock write failures

One reconcile loop serializes every wake-lock write: demand is recomputed after
each completed sysfs write, and only a completed write changes what the kernel
is believed to hold. A missing live sysfs node is an error, not a no-op
success. A failed restore retries a bounded number of times and then retains
the mismatch for the next explicit resume or acquire. A Full acquire whose
kernel protection cannot be confirmed fails instead of letting critical work
run unprotected.

Lease registration and drops only update intent and notify a control worker, so
event dispatch never waits on sysfs; shutdown awaits the final release rather
than writing on the dropping thread.

During an Explicit suspend sleep window,
<a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html#method.pause_soft_suspend_wake_lock">`pause`</a>
releases the shared lock immediately, bypassing autosleep grace, while keeping
every lease tracked.
<a href="/api/cadmus_core/device/inhibitor/struct.Inhibitor.html#method.resume_soft_suspend_wake_lock">`resume`</a>
re-locks only if a holder remains. Suppression is refused while a Full lease is
live, and a Full acquire during the window re-locks `cadmus` at once;
registering the nested lease alone only cancels a pending unlock.

## Deferred Explicit suspend

While any Full lease is held,
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>
records a Deferred Explicit suspend and returns without a cycle.
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle_from_auto.html">`start_cycle_from_auto`</a>
also defers while a Sleep lease is held.

A Full lease acquired after a cycle starts aborts it before kernel sleep during
preparation or a DeepIdle wait. The Classic case is different: the owned commit
holds a Full suspend gate from its final recheck through `mem`, wake, peripheral
cleanup, and wake-lock restoration, and a Full acquire takes the same gate. A
pending Full request counts as held from before it waits, so the commit's
rechecks see it and back off; a request that arrives after the last recheck
cannot interrupt the write and aborts only after wake and cleanup.

The abort preserves the cycle's origin and any newer queued request, with
Explicit winning over Auto, so an Explicit cycle cut short by Full remains a
Deferred Explicit suspend. If Full is already cleared the intent is flushed
immediately; otherwise it waits for
<a href="/api/cadmus_core/view/enum.Event.html#variant.InhibitCleared">`Event::InhibitCleared(Kind::Full)`</a>.
That release flushes a Deferred Auto Suspend through `start_cycle_from_auto`
and a Deferred Explicit suspend through `start_cycle`, even if a Sleep lease is
still held. The last Sleep release flushes only a Deferred Auto Suspend; it
re-defers while a Full or another Sleep lease is held, and leaves a Deferred
Explicit suspend queued for the Full release. A release during a running cycle
starts nothing.

User activity clears deferred intent: a touch calls
<a href="/api/cadmus_core/device/fn.reschedule_auto_suspend_alarm.html">`reschedule_auto_suspend_alarm`</a>,
which drops a Deferred Explicit suspend (and any Deferred Auto Suspend) without
starting a cycle.

OTA success posts
<a href="/api/cadmus_core/view/enum.Event.html#variant.ClearDeferredSuspend">`Event::ClearDeferredSuspend`</a>
before releasing `"ota"` so deferred intent does not race the reboot. Clearing
does **not** re-arm
<a href="/api/cadmus_core/device/rtc/enum.AlarmType.html#variant.AutoSuspend">`AutoSuspend`</a>;
a later `reschedule_auto_suspend_alarm` must do that.

### Release before reboot

A holder that reboots must drop Full **first**, then send the reboot event. OTA
sends `ClearDeferredSuspend`, drops its `"ota"` guard, then schedules the
reboot.

## Status LED arbiter

<a href="/api/cadmus_core/device/leds/struct.StatusLed.html">`StatusLed`</a>
multiplexes one hardware LED, and the higher
<a href="/api/cadmus_core/device/leds/enum.LedPriority.html">`LedPriority`</a>
wins. Full pulses at `LedPriority::FullInhibit`; the soft-suspend indicator is
solid at the lower `LedPriority::SoftIndicate`. A Sleep lease does not drive the
LED.

User-facing explanation: [Soft Suspend § Status LED](../../../soft-suspend.md#status-led-indicator).

<!-- i18n:skip-end -->
