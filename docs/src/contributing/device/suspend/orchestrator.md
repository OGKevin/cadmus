<!-- i18n:skip-start -->

# Suspend orchestrator

**Explicit** suspend: intermission UI, prepare teardown, then kernel sleep.
Shared code in
<a href="/api/cadmus_core/device/suspend/index.html">`device::suspend`</a>
(`crates/core/src/device/suspend/`).

This is separate from opportunistic [soft suspend](soft-suspend.md) and from
[Kobo platform hooks](kobo/suspend.md). To **block** starting a cycle, use
[Inhibitor `Kind::Full`](inhibitor.md). [`Kind::Sleep`](inhibitor.md) defers
only the Auto Suspend path (`start_cycle_from_auto`). SoftSuspend leases alone
do not gate `start_cycle`.

The emulator does not drive this module today.

## Workflow

Callers that mean “go to sleep” (power button, Auto Suspend RTC, sleep cover)
call
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_cycle.html">`start_cycle`</a>,
**not**
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.enter_sleep.html">`enter_sleep`</a>.

```mermaid
sequenceDiagram
    actor U as Requester
    participant O as Orchestrator
    participant I as Inhibitor / wake lock
    participant P as PowerManager
    participant K as Kernel

    U->>O: Suspend request
    O->>I: Check Full
    alt Full active
        O-->>U: Defer Auto or Explicit intent
        U->>O: FullInhibitCleared replays saved intent
    else Allowed
        O->>O: start_cycle(): create cycle and intermission UI
        O->>O: PrepareSuspend: flush work and stage WiFi / RTC
        alt Cancel during preparation
            O->>O: finish_cycle() without recording a wake
        else Continue
            alt Classic
                O->>P: Start owned commit: prepare peripherals
                alt Preparation failure or cancel before the gate
                    P-->>O: NotSlept and peripheral cleanup
                    O->>O: Schedule retry debounce without wake classification
                else Gate wait
                    P->>I: Acquire gate and recheck Full
                    Note over P,I: A Full acquire waits while the gate is held
                    alt Full seen at gate recheck
                        P-->>O: BlockedByFull
                        O->>O: finish_blocked_cycle() and restore interactive
                    else Gate held
                        P->>I: Pause wake lock
                        alt Pause refused by Full
                            P-->>O: BlockedByFull
                            O->>O: finish_blocked_cycle() and restore interactive
                        else Unlock failure
                            P-->>O: NotSlept and peripheral cleanup
                            O->>O: Schedule retry debounce without wake classification
                        else Paused
                            P->>K: Write mem
                            alt mem failure
                                K-->>P: Write error
                                P-->>O: NotSlept after resume, wake-lock restore and cleanup
                                O->>O: Schedule retry debounce without wake classification
                            else Kernel sleep and wake
                                K-->>P: mem returns after wake
                                P-->>O: Capture clocks, clean up peripherals, restore wake lock and release gate
                                alt Full still held after wake
                                    O->>O: finish_blocked_cycle() and restore interactive
                                else No Full
                                    O->>O: Classify wake and enter PostWakeDebounce
                                end
                            end
                        end
                    end
                end
            else DeepIdle
                O->>P: Force autosleep mem and arm peripherals
                O->>I: Pause wake lock and store epoch
                alt Refused or unlock failure
                    I-->>O: BlockedByFull or UnlockFailed
                    O-->>U: Restore without recording a wake
                else Paused
                    loop PollDeepIdleWait until wake, interruption or timeout
                        O->>O: Compare clocks and check pause epoch
                        alt Full invalidates epoch
                            O->>O: finish_blocked_cycle() and restore interactive
                        else Wake detected
                            O->>O: Classify wake and enter PostWakeDebounce
                        else Timeout
                            O->>P: Retry same cycle while allowed, or fall back to Classic after three
                        end
                    end
                end
            end
        end
        opt Wake recorded
            O->>O: Restore wake lock once when a saved pause epoch is present
            O->>O: Restore peripherals while retaining the cycle
            O->>O: PostWakeDebounce
            alt User activity or cancellation
                O->>O: finish_cycle()
                O->>O: Restore frontlight and intended WiFi state
                O-->>U: Interactive
            else WakeDebounce or CalendarUpdate
                O->>O: Re-enter sleep with the same cycle kind
            end
        end
    end
```

| Function            | Role                                                    |
| ------------------- | ------------------------------------------------------- |
| `start_cycle`       | Begin cycle: UI + schedule prepare                      |
| `prepare_for_sleep` | Teardown; arm Classic Suspend RTC or enter DeepIdle     |
| `enter_sleep`       | Kernel sleep (owned Classic commit or deep-idle poll)   |
| `finish_cycle`      | Tear down → interactive                                 |

`finish_cycle` is the only teardown path: it cancels the Suspend and
WakeDebounce RTCs, clears the prepare/poll tasks, drops the cycle lease,
restores the saved autosleep mode, disarms kernel deep idle, and restores the
wake lock only when the exiting sleep leg still owns suppression, so one leg
never restores twice. Cancelling while only a PrepareSuspend task is pending
still restores frontlight and WiFi when a replacement cycle already ran
teardown.

<a href="/api/cadmus_core/view/enum.Event.html#variant.Suspend">`Event::Suspend`</a>
/
<a href="/api/cadmus_core/device/rtc/enum.AlarmType.html#variant.Suspend">`AlarmType::Suspend`</a>
mean **enter sleep now** (after prepare), not “start a new cycle”.

Full inhibit defers `start_cycle` as Explicit origin. If a worker acquires Full
during preparation or the deep-idle wait, the orchestrator aborts the cycle
before kernel sleep. Classic timing is different: the owned commit takes the
Full suspend gate for its final Full recheck and holds it through `mem`, wake,
peripheral cleanup, and wake-lock restoration, and `Kind::Full` acquisition
takes the same gate. A Full acquire that arrives while the gate is held waits
for it and cannot interrupt the write; if Full is still held after wake and
cleanup, the cycle aborts then. A Full holder acquired before the gate is seen
by the recheck (or by the refused pause) and aborts before `mem`.

An abort preserves the current cycle's origin and any newer queued request,
with Explicit winning over Auto. If Full has already cleared, the intent is
flushed immediately, because `InhibitCleared(Kind::Full)` may already have been
handled. Otherwise it waits for `InhibitCleared(Kind::Full)`, which flushes Auto
through `start_cycle_from_auto` and Explicit through `start_cycle`. A start
while a suspend cycle is in flight is ignored. The last Sleep release flushes a
deferred Auto through `start_cycle_from_auto`, which re-defers while a Full or
Sleep lease is held, and leaves a deferred Explicit for the Full release. All
SoftSuspend holders, including the nested `"sleep-inhibit"` lease, remain
tracked while the shared `cadmus` wake lock is suppressed for the kernel-sleep
window; wake restores it when any holder remains. Suppression is refused while
a Full holder is live. If Full arrives during a DeepIdle window, its nested
wake lease immediately restores `cadmus` and the cycle aborts before sleep — [Inhibitor § Deferred explicit suspend](inhibitor.md#deferred-explicit-suspend).

## Kind and phase

`Option<`<a href="/api/cadmus_core/device/suspend/cycle/struct.SuspendCycle.html">`SuspendCycle`</a>`>`
on
<a href="/api/cadmus_core/context/struct.Context.html#structfield.suspend">`Context::suspend`</a>.
Interactive use is `None`.

| Field   | Values                                                                                                                                         |
| ------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `kind`  | `Classic` or `DeepIdle` — set at `start_cycle`; becomes `Classic` when a deep-idle cycle falls back after three consecutive timed-out attempts |
| `phase` | `Preparing` → `ArmingSleep` → `PostWakeDebounce`; DeepIdle also uses `InSleep` while polling                                                   |

Do not re-pick Classic vs DeepIdle mid-cycle from
<a href="/api/cadmus_core/device/soft_suspend/mode/enum.AutosleepMode.html#method.is_armed">`is_armed()`</a>
alone. WakeDebounce / CalendarUpdate re-enter with the same kind.

## Sleep backends

| Kind       | Entry                                                             |
| ---------- | ----------------------------------------------------------------- |
| `Classic`  | Suspend RTC, then an owned `prepare_suspend` → `suspend` commit   |
| `DeepIdle` | Force autosleep `mem`, `arm_deep_idle`, poll until wake / timeout |

`PowerManager` is async end to end, but the sysfs and `sync()` work it submits
is genuinely blocking and runs on the runtime's capped blocking pool
(`tokio::fs` writes and `spawn_blocking`), not on the reactor workers.
`prepare_suspend` arms peripherals, awaits the settle timer, and flushes
buffers; it does not enter kernel sleep. `suspend` is only the `state=mem`
write, whose future resolves when that write returns, that is after wake. One
owned Classic job runs preparation, final arbitration and pause, the write,
resume, wake-lock restoration, and peripheral cleanup. The owned gate token
stays held through cleanup, so a dropped waiter cannot release it or cancel a
submitted write; cycle finish and shutdown drain the commit before further
hardware work. Cancellation is cooperative only before the gate token is
acquired. A Full block, preparation failure, unlock failure, or `mem` failure
is **not** a wake: only successful `mem` runs post-wake alarm classification.

DeepIdle snapshots in two steps rather than at one instant. The alarm state is
captured before autosleep is forced to `mem` and peripherals are armed. After
arming and `sync()`, the interrupt baseline, the wall-clock `sleep_started_at`,
and the boottime and monotonic anchors are captured immediately before the
wake-lock pause. Only `sleep_started_at` is used for post-wake RTC alarm
claims; the boottime and monotonic anchors drive wake detection. Each poll then
checks the stored suppression epoch. A Full holder that arrives and leaves
between pause and the next poll still aborts the cycle rather than reporting a
timeout or synthetic wake.

RTC transactions, display-update waits, and lease protection that needs kernel
confirmation run on the blocking pool, but event dispatch stays serial: a
cover-open queued during Classic preparation waits for the current handler,
not an interrupt-driven cancel. Last-moment Full checks still happen at deep
idle pause and the Classic commit boundary.

Both share prepare teardown and schedule WakeDebounce after wake. AutoPowerOff
and CalendarUpdate alarms are scheduled before sleep when their settings
require them, then checked after wake. Platform details:
[Kobo suspend](kobo/suspend.md).

## Deep-idle wake detect

Production: `CLOCK_BOOTTIME` − `CLOCK_MONOTONIC` (~1 s threshold). Tests inject
<a href="/api/cadmus_core/device/suspend/wake/enum.PollResult.html">`PollResult`</a>
via `AppContext::deep_idle_poll_inject`.

On timeout, the wait re-arms while soft suspend can re-arm; otherwise
`finish_cycle`. DeepIdle stops after three consecutive timed-out attempts: the
cap counts consecutive timeouts on `SuspendCycle`, so a cycle can make more than
three deep-idle attempts when wakes interleave. A detected wake resets the
count, and the WakeDebounce re-sleep starts fresh. Past the cap the cycle falls
back to `Classic` and disarms autosleep (`Off`) for the rest of the cycle,
because the kernel rejects `state=mem` while autosleep is armed, which would
turn every attempt into the same guard error; `finish_cycle` restores the
stashed mode. The cycle then stays on the classic WakeDebounce re-sleep path,
whose errno is visible, instead of polling autosleep every 30 s.

The first timeout of a failing cycle also emits one `deep idle wait timed out;
suspend blockers` warning naming the lease holders, `/sys/power/wake_lock`,
`/sys/power/wakeup_count` (read under a short timeout), the charger/USB state,
and the IRQ delta. A refused `Classic` suspend adds the same kernel-side fields
to `Failed to suspend device`.

## Wake-source telemetry

On Linux,
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.start_deep_idle_wait.html">`start_deep_idle_wait`</a>
snapshots the pre-sleep alarm state before
<a href="/api/cadmus_core/device/power/trait.PowerManager.html#method.arm_deep_idle">`arm_deep_idle`</a>
and the IRQ counters after arming and `sync()`, immediately before the
wake-lock pause and the cycle lease drop, storing both on
<a href="/api/cadmus_core/device/suspend/cycle/struct.DeepIdleWaitState.html">`DeepIdleWaitState`</a>:

| Captured               | Source                                                    |
| ---------------------- | --------------------------------------------------------- |
| hardware wake alarm    | `Rtc::wake_alarm` → `/sys/class/rtc/rtc0/wakealarm`       |
| earliest logical alarm | `AlarmManager::next_scheduled_alarm` (future alarms only) |
| IRQ counters           | `/proc/interrupts`, sampled before the wake-lock pause    |

Each detected wake then emits one `Primary deep-idle wake` record from
<a href="/api/cadmus_core/device/suspend/orchestrator/fn.log_wake_source.html">`log_wake_source`</a>,
carrying `pre_hardware_alarm`, `pre_cadmus_alarm_type`, `pre_cadmus_alarm`,
`post_hardware_alarm`, `wake_irq`, `wake_irq_name`, `unmanaged_alarm_candidate`,
`irq_delta_status`, `irq_deltas_json` and `irq_delta_reasons_json`. Sampling
`/proc/interrupts` after wake, before the other post-wake node reads, also
supplies the existing `wake_irq_name`; each retry samples a new baseline.

What the fields mean, and what they cannot say:

- `wake_irq` is `/sys/power/pm_wakeup_irq`, read inside the wake path because the
  kernel clears it on the next suspend. `-ENODATA` means the kernel recorded no
  IRQ. Other read errors and malformed content also log `wake_irq = None`, so a
  missing value means "not reported", not that no hardware wake happened.
- `wake_irq_name` is the owning row in `/proc/interrupts`. A parent controller
  such as the PMIC (`bd71828_irq`) does not identify its own child, so compare
  per-IRQ counters before naming a source.
- `unmanaged_alarm_candidate` is true when the hardware slot and the earliest
  future logical alarm disagree. The kernel records no writer for the slot, so a
  mismatch is a candidate for an unmanaged alarm, never proof of who wrote it.
- `irq_deltas_json` is a JSON-encoded array of `{irq, name, delta}` objects. `irq`
  is a string; `name` is the post-wake action, or `null` for rows without one.
  `before_name` is present when the baseline action was present and differs from
  the post-wake action, including when the post-wake snapshot has no action. Each
  delta sums
  increases across the CPUs for one IRQ. Non-numeric summary rows (`IPI*`, `ERR`,
  `NMI`) are excluded from the comparison. Decode it with
  `.fields.irq_deltas_json | fromjson`. Parent and child rows appear independently.
  Changed counters are evidence of activity between the samples, not proof of wake
  causation; they can also include activity before sleep and after wake.
- `irq_delta_reasons_json` explains a `partial` status: a JSON array of
  `{irq, reason}` objects. Reasons are `missing_before`, `missing_after`,
  `unparseable_before`, `unparseable_after` and `counter_not_monotonic`. It is
  `[]` for `complete` and `unavailable`.
- `irq_delta_status` is `complete` for a successful comparison, including no changes,
  `partial` when unusable rows, rows added, removed, or duplicated, or
  counter decreases/overflows make the coverage incomplete, or `unavailable` when a
  snapshot, valid CPU header, or numeric row data cannot support a comparison, including
  CPU columns that differ between samples. Only positive deltas are emitted; missing or
  incompatible data does not become a zero.
- `next_scheduled_alarm` ignores past-due alarms, matching what
  `update_hardware_alarm_at` programs into the slot.

Non-Linux targets still record the earliest logical alarm, but the hardware slot
reads `Unavailable` and nothing is emitted.

## Phase-aware Suspend

| Situation                                                  | Action                          |
| ---------------------------------------------------------- | ------------------------------- |
| `Suspend` during `InSleep` / `PostWakeDebounce`            | Ignore                          |
| Interactive + soft armed + classic `Suspend` without cycle | Refuse; reschedule Auto Suspend |

<!-- i18n:skip-end -->
