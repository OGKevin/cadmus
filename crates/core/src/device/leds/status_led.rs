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
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Visual pattern driven on the physical status LED.
///
/// Used with [`StatusLed::install`]. Blink timings are interpreted by the arbiter
/// worker thread, not tied to the main loop.
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
    /// Set when [`StatusLed`] is dropped so the worker exits.
    shutdown: bool,
}

/// Why the pattern worker stopped waiting.
enum WaitOutcome {
    /// Commands changed; re-evaluate the winner.
    Changed,
    /// Shutdown was requested, or a blink phase timed out without a change.
    Shutdown,
}

struct StatusLedInner {
    /// Physical LED backend; `None` when hardware is unavailable.
    leds: Option<Arc<dyn DeviceLeds>>,
    /// Active commands and worker coordination flags.
    state: Mutex<ArbiterState>,
    /// Wakes the pattern worker after installs, releases, or shutdown.
    cv: Condvar,
    /// Source of [`LedCommandSequence`] values for equal-priority tie-breaks.
    sequence: AtomicU64,
}

/// Drives the status LED from prioritized named commands.
///
/// Construct once per [`Inhibitor`](crate::device::inhibitor::Inhibitor) and
/// share the `Arc` across autosleep policy and future Full-inhibit wiring.
pub struct StatusLed {
    inner: Arc<StatusLedInner>,
    worker: Option<thread::JoinHandle<()>>,
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
    fn run(self: &Arc<Self>) {
        loop {
            let (pattern, generation) = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.shutdown {
                    break;
                }
                (self.winning_pattern(&state), state.generation)
            };

            match pattern {
                None => {
                    self.write_led(false);
                    match self.wait_for_change(generation, None) {
                        WaitOutcome::Changed => continue,
                        WaitOutcome::Shutdown => break,
                    }
                }
                Some(LedPattern::SolidOn) => {
                    self.write_led(true);
                    match self.wait_for_change(generation, None) {
                        WaitOutcome::Changed => continue,
                        WaitOutcome::Shutdown => break,
                    }
                }
                Some(LedPattern::SolidOff) => {
                    self.write_led(false);
                    match self.wait_for_change(generation, None) {
                        WaitOutcome::Changed => continue,
                        WaitOutcome::Shutdown => break,
                    }
                }
                Some(LedPattern::Blink { on, off }) => {
                    self.write_led(true);
                    match self.wait_for_change(generation, Some(on)) {
                        WaitOutcome::Changed => continue,
                        WaitOutcome::Shutdown => {}
                    }
                    self.write_led(false);
                    match self.wait_for_change(generation, Some(off)) {
                        WaitOutcome::Changed => continue,
                        WaitOutcome::Shutdown => {}
                    }
                }
            }
        }
        self.write_led(false);
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

    fn wait_for_change(&self, generation: u64, timeout: Option<Duration>) -> WaitOutcome {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.shutdown {
                return WaitOutcome::Shutdown;
            }
            if state.generation != generation {
                return WaitOutcome::Changed;
            }
            state = match timeout {
                Some(duration) => {
                    let (guard, wait_result) = self
                        .cv
                        .wait_timeout(state, duration)
                        .unwrap_or_else(|e| e.into_inner());
                    if wait_result.timed_out() {
                        return if guard.generation != generation {
                            WaitOutcome::Changed
                        } else {
                            WaitOutcome::Shutdown
                        };
                    }
                    guard
                }
                None => self.cv.wait(state).unwrap_or_else(|e| e.into_inner()),
            };
        }
    }
}

impl StatusLed {
    /// Creates an arbiter over `leds` and starts the pattern worker thread.
    ///
    /// Pass `None` when hardware is unavailable; installs still succeed for tests
    /// and noop hosts.
    pub fn new(leds: Option<Arc<dyn DeviceLeds>>) -> Arc<Self> {
        let inner = Arc::new(StatusLedInner {
            leds,
            state: Mutex::new(ArbiterState {
                commands: HashMap::new(),
                generation: 0,
                shutdown: false,
            }),
            cv: Condvar::new(),
            sequence: AtomicU64::new(0),
        });
        let worker = Arc::clone(&inner);
        let handle = thread::spawn(move || worker.run());
        Arc::new(Self {
            inner,
            worker: Some(handle),
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
            state.generation = state.generation.wrapping_add(1);
        }
        self.inner.cv.notify_one();
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
            if removed {
                state.generation = state.generation.wrapping_add(1);
            }
            removed
        };
        if removed {
            self.inner.cv.notify_one();
        }
    }
}

impl Drop for StatusLed {
    fn drop(&mut self) {
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.shutdown = true;
        }
        self.inner.cv.notify_one();
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
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

    fn wait_for<F: Fn() -> bool>(predicate: F) {
        for _ in 0..200 {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("condition not met within timeout");
    }

    #[test]
    fn higher_priority_overrides_lower() {
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
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);

        let _high = status_led.install(
            "full-inhibit",
            LedPriority::FullInhibit,
            LedPattern::Blink {
                on: Duration::from_millis(20),
                off: Duration::from_millis(20),
            },
        );
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn reverts_after_higher_release() {
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
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);
        let high = status_led.install(
            "full-inhibit",
            LedPriority::FullInhibit,
            LedPattern::SolidOff,
        );
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1);
        drop(high);
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn replace_same_name_updates_pattern() {
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
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);
        let second = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOff,
        );
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1);
        drop(first);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            leds.on_calls.load(Ordering::SeqCst),
            1,
            "stale guard must not remove the replaced command"
        );
        drop(second);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn empty_map_turns_led_off() {
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
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);
        drop(guard);
        wait_for(|| leds.off_calls.load(Ordering::SeqCst) >= 1);
    }

    #[test]
    fn missing_hardware_succeeds_without_io() {
        let status_led = StatusLed::new(None);
        let guard = status_led.install(
            "soft-indicate",
            LedPriority::SoftIndicate,
            LedPattern::SolidOn,
        );
        drop(guard);
    }

    #[test]
    fn drop_joins_worker_and_turns_led_off() {
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
        wait_for(|| leds.on_calls.load(Ordering::SeqCst) >= 1);
        drop(guard);
        drop(status_led);
        assert!(
            leds.off_calls.load(Ordering::SeqCst) >= 1,
            "drop must join the worker after the final LED-off write"
        );
    }
}
