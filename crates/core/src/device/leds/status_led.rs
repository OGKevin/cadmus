//! Prioritized status-LED command arbiter.
//!
//! Multiple subsystems may want the physical status LED at once (soft-suspend
//! indicate, Full inhibit blink, future signals). [`StatusLed`] accepts named
//! commands with priorities; a background worker drives the hardware from the
//! current winner. Equal priority resolves to the most recently installed command.
//!
//! Missing [`DeviceLeds`] hardware is handled gracefully: installs succeed and
//! drops still run, but no sysfs or GPIO writes occur.

use super::DeviceLeds;
use super::LedPriority;
use crate::lease::LeaseName;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Visual pattern driven on the physical status LED.
///
/// Used with [`StatusLed::install`]. Blink timings are interpreted by the arbiter
/// worker task, not tied to the main loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedPattern {
    /// LED steady on.
    SolidOn,
    /// LED steady off.
    SolidOff,
    /// LED alternates on and off for the given durations.
    Blink {
        /// How long the LED stays on in each blink cycle.
        on: Duration,
        /// How long the LED stays off in each blink cycle.
        off: Duration,
    },
}

/// Monotonic install counter used as an equal-priority tie-breaker.
///
/// Higher values win when two commands share the same [`LedPriority`]. Assigned
/// by [`StatusLed::install`]; callers never set it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct LedCommandSequence(u64);

impl LedCommandSequence {
    fn next(counter: &AtomicU64) -> Self {
        Self(counter.fetch_add(1, Ordering::Relaxed))
    }
}

/// One named claim registered with the status-LED arbiter.
struct LedCommand {
    /// Priority tier; higher wins while multiple commands are active.
    priority: LedPriority,
    /// Pattern driven when this command is the winner.
    pattern: LedPattern,
    /// Install order; breaks ties when priorities are equal (higher wins).
    sequence: LedCommandSequence,
}

/// Shared mutable state for the arbiter and its worker thread.
struct ArbiterState {
    /// Active commands keyed by lease name.
    commands: HashMap<LeaseName, LedCommand>,
    /// Bumped on every install/release so the worker can detect changes.
    generation: u64,
}

struct StatusLedInner {
    /// Physical LED backend; `None` when hardware is unavailable.
    leds: Option<Arc<dyn DeviceLeds>>,
    /// Active commands and the generation counter.
    state: Mutex<ArbiterState>,
    /// Carries both "the command set changed" and "shut down" to the worker.
    ///
    /// A `watch` rather than a `Condvar` so the worker is an ordinary async
    /// task on the runtime instead of a blocking-pool thread: the process
    /// runtime has two workers, and a parked LED arbiter is a thread it cannot
    /// reclaim.
    signal: watch::Sender<LedSignal>,
    /// Source of [`LedCommandSequence`] values for equal-priority tie-breaks.
    sequence: AtomicU64,
}

/// What changed since the worker last looked at the command set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LedSignal {
    /// A new install or release landed; the generation is `n`.
    Changed(u64),
}

/// What ended one blink phase.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlinkPhaseOutcome {
    /// The phase's duration elapsed; flip the LED and run the next phase.
    Elapsed,
    /// A new install or release landed; re-evaluate the winner.
    Changed,
    /// The owner is gone, or shutdown was signalled; stop the worker.
    Stopped,
}

/// Drives the status LED from prioritized named commands.
///
/// Construct once per [`Inhibitor`](crate::device::inhibitor::Inhibitor) and
/// share the `Arc` across autosleep policy and future Full-inhibit wiring.
pub struct StatusLed {
    inner: Arc<StatusLedInner>,
    /// Owned here so the worker cannot outlive the arbiter. `Drop` cancels it
    /// rather than joining.
    worker: Option<crate::runtime::Job<()>>,
}

/// RAII guard for an installed status-LED command.
///
/// Drop the guard to release `name` from the arbiter. If the released command
/// was winning, the worker re-evaluates and may revert to a lower-priority pattern.
pub struct StatusLedGuard {
    /// Lease name of the command this guard keeps registered.
    name: LeaseName,
    /// Install generation; drop removes the command only when this still matches.
    sequence: LedCommandSequence,
    /// Arbiter that owns the command map.
    status_led: Arc<StatusLed>,
}

impl StatusLedInner {
    /// Drives the LED until the owning [`Job`] is cancelled, then leaves it off.
    ///
    /// Async on purpose: the worker lives for the process, and as a
    /// blocking-pool thread it would be a thread the two-worker runtime cannot
    /// reclaim. The solid case parks on the `watch` channel; only a blink has
    /// a timed phase, so only a blink holds a runtime timer.
    ///
    /// Each iteration snapshots the winning pattern and generation together.
    /// If a [`LedSignal::Changed`] arrived after that snapshot, its generation
    /// will not match and the loop re-snapshots without driving stale output.
    async fn run(
        self: Arc<Self>,
        mut signal: watch::Receiver<LedSignal>,
        job_cancel: CancellationToken,
    ) {
        loop {
            let (pattern, generation) = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                (self.winning_pattern(&state), state.generation)
            };
            if matches!(*signal.borrow(), LedSignal::Changed(g) if g != generation) {
                tokio::select! {
                    _ = job_cancel.cancelled() => break,
                    changed = signal.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                }
                continue;
            }

            match pattern {
                None | Some(LedPattern::SolidOff) => {
                    self.write_led(false);
                    if self.wait_for_led_change(&mut signal, &job_cancel).await {
                        break;
                    }
                }
                Some(LedPattern::SolidOn) => {
                    self.write_led(true);
                    if self.wait_for_led_change(&mut signal, &job_cancel).await {
                        break;
                    }
                }
                Some(LedPattern::Blink { on, off }) => {
                    self.write_led(true);
                    match self.blink_phase(&mut signal, &job_cancel, on).await {
                        BlinkPhaseOutcome::Stopped => break,
                        BlinkPhaseOutcome::Changed => continue,
                        BlinkPhaseOutcome::Elapsed => {}
                    }
                    self.write_led(false);
                    match self.blink_phase(&mut signal, &job_cancel, off).await {
                        BlinkPhaseOutcome::Stopped => break,
                        BlinkPhaseOutcome::Changed => continue,
                        BlinkPhaseOutcome::Elapsed => {}
                    }
                }
            }
        }
        self.write_led(false);
    }

    async fn wait_for_led_change(
        &self,
        signal: &mut watch::Receiver<LedSignal>,
        job_cancel: &CancellationToken,
    ) -> bool {
        tokio::select! {
            _ = job_cancel.cancelled() => true,
            changed = signal.changed() => changed.is_err(),
        }
    }

    /// Runs one blink phase.
    ///
    /// A phase that elapses normally is not a stop: the worker flips the LED and
    /// starts the next phase. It stops only on shutdown, on a command change, or
    /// when the owning [`StatusLed`] is dropped mid-phase — the last matters
    /// because a dropped sender makes `changed()` fail, which is also how the
    /// phase ends.
    ///
    /// If the phase timer wins `select!` while a command change is already pending
    /// on the watch channel, the outcome is [`BlinkPhaseOutcome::Changed`], not
    /// [`BlinkPhaseOutcome::Elapsed`], so the worker does not run the opposite
    /// blink phase on stale output.
    async fn blink_phase(
        &self,
        signal: &mut watch::Receiver<LedSignal>,
        job_cancel: &CancellationToken,
        phase: Duration,
    ) -> BlinkPhaseOutcome {
        // `Ok(Err(_))`: the sender dropped. `Ok(Ok(()))`: a change landed.
        // `Err(_)` of the timeout: the phase elapsed, which is the normal path.
        let result = tokio::select! {
            _ = job_cancel.cancelled() => return BlinkPhaseOutcome::Stopped,
            changed = tokio::time::timeout(phase, signal.changed()) => changed,
        };
        let Ok(result) = result else {
            if signal.has_changed().unwrap_or(false) {
                signal.borrow_and_update();
                return BlinkPhaseOutcome::Changed;
            }
            return BlinkPhaseOutcome::Elapsed;
        };
        match result {
            Err(_) => BlinkPhaseOutcome::Stopped,
            Ok(()) => {
                signal.borrow_and_update();
                BlinkPhaseOutcome::Changed
            }
        }
    }

    fn winning_pattern(&self, state: &ArbiterState) -> Option<LedPattern> {
        state
            .commands
            .values()
            .max_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then(left.sequence.cmp(&right.sequence))
            })
            .map(|command| command.pattern)
    }

    fn write_led(&self, on: bool) {
        let Some(leds) = self.leds.as_ref() else {
            return;
        };
        let result = if on { leds.on() } else { leds.off() };
        if let Err(error) = result {
            tracing::warn!(error = %error, on, "failed to write status LED");
        }
    }
}

impl StatusLed {
    /// Creates an arbiter over `leds` and starts the pattern worker task.
    ///
    /// Pass `None` when hardware is unavailable; installs still succeed for tests
    /// and noop hosts.
    pub fn new(leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        let (signal, receiver) = watch::channel(LedSignal::Changed(0));
        let inner = Arc::new(StatusLedInner {
            leds,
            state: Mutex::new(ArbiterState {
                commands: HashMap::new(),
                generation: 0,
            }),
            signal,
            sequence: AtomicU64::new(0),
        });
        let worker = Arc::clone(&inner);
        let job = crate::runtime::Job::spawn(move |job_cancel| worker.run(receiver, job_cancel));
        Arc::new(Self {
            inner,
            worker: Some(job),
        })
    }

    /// Installs or replaces `name` with `pattern` at `priority`.
    ///
    /// Returns a guard that keeps the command registered until dropped. Replacing
    /// an existing `name` updates its pattern and refresh order at the same priority.
    pub fn install(
        self: &Arc<Self>,
        name: impl Into<LeaseName>,
        priority: LedPriority,
        pattern: LedPattern,
    ) -> StatusLedGuard {
        let name = name.into();
        let sequence = LedCommandSequence::next(&self.inner.sequence);
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.commands.insert(
                name.clone(),
                LedCommand {
                    priority,
                    pattern,
                    sequence,
                },
            );
        }
        StatusLedGuard {
            name,
            sequence,
            status_led: Arc::clone(self),
        }
    }

    /// Releases `name` when the installed command still matches `sequence`.
    fn release(self: &Arc<Self>, name: &LeaseName, sequence: LedCommandSequence) {
        let removed = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let removed = match state.commands.get(name) {
                Some(command) if command.sequence == sequence => {
                    state.commands.remove(name).is_some()
                }
                _ => false,
            };
            removed
        };
        if removed {
            state.generation = state.generation.wrapping_add(1);
            let generation = state.generation;
            self.inner.signal.send(LedSignal::Changed(generation)).ok();
        }
    }
}

impl Drop for StatusLed {
    fn drop(&mut self) {
        // Cancel rather than join: the worker turns the LED off and exits on
        // its own, and a `Drop` that waits can abort the process while unwinding.
        if let Some(job) = self.worker.take() {
            job.cancel();
        }
    }
}

impl Drop for StatusLedGuard {
    fn drop(&mut self) {
        self.status_led.release(&self.name, self.sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::leds::LedsError;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingLeds {
        on_calls: AtomicU32,
        off_calls: AtomicU32,
    }

    impl DeviceLeds for CountingLeds {
        fn on(&self) -> Result<(), LedsError> {
            self.on_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn off(&self) -> Result<(), LedsError> {
            self.off_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Polls `predicate`, yielding to the runtime so the async worker is
    /// scheduled. A plain `thread::sleep` loop would starve it on a
    /// two-worker runtime, which is why this is `async`.
    async fn wait_for<F: Fn() -> bool>(predicate: F) {
        for _ in 0..200 {
            if predicate() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition not met within timeout");
    }

    /// A command change during the on phase must apply before the off phase runs.
    ///
    /// Installs the override from the first `on()` callback so a loaded host
    /// cannot start another blink cycle before the higher-priority pattern lands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blink_applies_command_change_before_opposite_phase() {
        let override_guard = Arc::new(Mutex::new(None));
        let status_led = Arc::new(Mutex::new(None::<Arc<StatusLed>>));
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
            on_first_on: Some(Box::new({
                let status_led = Arc::clone(&status_led);
                let override_guard = Arc::clone(&override_guard);
                move || {
                    let led = status_led.lock().unwrap();
                    let led = led.as_ref().expect("status LED installed");
                    *override_guard.lock().unwrap() = Some(led.install(
                        "full-inhibit",
                        LedPriority::FullInhibit,
                        LedPattern::SolidOff,
                    ));
                }
            })),
        });
        let led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        *status_led.lock().unwrap() = Some(Arc::clone(&led));
        let _blink = led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::Blink {
                on: Duration::from_secs(60),
                off: Duration::from_millis(200),
            },
        );

        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1).await;
        assert!(override_guard.lock().unwrap().is_some());
        assert_eq!(
            leds.on_calls.load(Ordering::SeqCst),
            1,
            "a command change during the on phase must not run the off phase first"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blink_keeps_toggling_across_multiple_phases() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
            on_first_on: None,
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let _blink = status_led.install(
            "full-inhibit",
            LedPriority::FullInhibit,
            LedPattern::Blink {
                on: Duration::from_millis(20),
                off: Duration::from_millis(20),
            },
        );

        // Wait for both counters: observing `on` alone is racy because `write_led(true)`
        // at the start of a cycle can run before the previous cycle's `write_led(false)`.
        wait_for(|| {
            leds.on_calls.load(Ordering::SeqCst) >= 3 && leds.off_calls.load(Ordering::SeqCst) >= 2
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn higher_priority_overrides_lower() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let _low = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;

        let _high = status_led.install(
            "full-inhibit",
            LedPriority::FullInhibit,
            LedPattern::Blink {
                on: Duration::from_millis(20),
                off: Duration::from_millis(20),
            },
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reverts_after_higher_release() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let _low = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;
        let high = status_led.install(
            "full-inhibit",
            LedPriority::FullInhibit,
            LedPattern::SolidOff,
        );
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1).await;
        drop(high);
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replace_same_name_updates_pattern() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let first = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;
        let second = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOff,
        );
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1).await;
        drop(first);

        // Prove the stale drop was a no-op by installing a sentinel: once the
        // worker applies it, we know it has already processed the drop. A fixed
        // sleep instead of this would be flaky under load.
        let on_before_sentinel = leds.on_calls.load(Ordering::SeqCst);
        let sentinel =
            status_led.install("sentinel", LedPriority::SoftIndicate, LedPattern::SolidOn);
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) > on_before_sentinel).await;
        assert_eq!(
            leds.on_calls.load(Ordering::SeqCst),
            on_before_sentinel + 1,
            "stale guard must not remove the replaced command"
        );
        drop(sentinel);
        drop(second);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 2).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_map_turns_led_off() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let guard = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;
        drop(guard);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_hardware_succeeds_without_io() {
        let status_led = StatusLed::new(None);
        let guard = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        drop(guard);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drop_signals_shutdown_and_worker_turns_led_off() {
        let leds = Arc::new(CountingLeds {
            on_calls: AtomicU32::new(0),
            off_calls: AtomicU32::new(0),
        });
        let status_led = StatusLed::new(Some(leds.clone() as Arc<dyn DeviceLeds>));
        let guard = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1).await;
        drop(guard);
        drop(status_led);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1).await;
    }
}
