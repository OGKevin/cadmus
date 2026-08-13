//! In-memory emulator RTC with Condvar-backed alarm IRQ waits.

use anyhow::Error;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::device::rtc::{Rtc, RtcWkalrm};

const RTC_AF: u32 = 0x20;

#[derive(Debug)]
struct EmulatorRtcState {
    /// Whether the in-memory hardware wake alarm is armed.
    alarm_enabled: bool,
    /// Programmed wake instant on the emulated RTC timeline, if any.
    alarm_wake_time: Option<DateTime<Utc>>,
    /// Set when an IRQ should unblock [`Rtc::wait_for_alarm_irq`].
    irq_pending: bool,
    /// Emulated RTC timeline as `system_now + offset` (see [`Rtc::read_time`]).
    offset: ChronoDuration,
    /// `RTC_now − system_now`; for the emulator this equals [`Self::offset`].
    ///
    /// Positive means the emulated RTC reads ahead of civil time.
    drift: ChronoDuration,
    /// `new_time − old_time` from the latest [`Rtc::set_time`], if not yet taken.
    pending_step: Option<ChronoDuration>,
}

/// Emulator RTC that schedules in-memory wake alarms and wakes IRQ waiters.
#[derive(Clone)]
pub struct EmulatorRtc {
    state: Arc<Mutex<EmulatorRtcState>>,
    cond: Arc<Condvar>,
}

impl EmulatorRtc {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(EmulatorRtcState {
                alarm_enabled: false,
                alarm_wake_time: None,
                irq_pending: false,
                offset: ChronoDuration::zero(),
                drift: ChronoDuration::zero(),
                pending_step: None,
            })),
            cond: Arc::new(Condvar::new()),
        }
    }
}

impl Default for EmulatorRtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rtc for EmulatorRtc {
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        let wake_time = state.alarm_wake_time.unwrap_or_else(Utc::now);
        Ok(RtcWkalrm::from_parts(state.alarm_enabled, wake_time))
    }

    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = true;
        state.alarm_wake_time = Some(wake_time);
        self.cond.notify_all();
        Ok(0)
    }

    fn disable_alarm(&self) -> Result<i32, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = false;
        self.cond.notify_all();
        Ok(0)
    }

    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        self.state
            .lock()
            .map(|state| Utc::now() + state.offset)
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))
    }

    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let system_now = Utc::now();
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        let old_time = system_now + state.offset;
        state.offset = time.signed_duration_since(system_now);
        state.drift = state.offset;
        state.pending_step = Some(time.signed_duration_since(old_time));
        self.cond.notify_all();
        Ok(())
    }

    fn drift(&self) -> Result<ChronoDuration, Error> {
        self.state
            .lock()
            .map(|state| state.drift)
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))
    }

    fn take_pending_step(&self) -> Result<Option<ChronoDuration>, Error> {
        self.state
            .lock()
            .map(|mut state| state.pending_step.take())
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))
    }

    fn wait_for_alarm_irq(&self, timeout: Option<Duration>) -> Result<Option<u32>, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        let deadline = timeout.map(|duration| std::time::Instant::now() + duration);

        loop {
            if state.irq_pending {
                state.irq_pending = false;
                return Ok(Some(RTC_AF));
            }

            let now = Utc::now() + state.offset;
            if state.alarm_enabled
                && let Some(wake_time) = state.alarm_wake_time
                && wake_time <= now
            {
                state.alarm_enabled = false;
                return Ok(Some(RTC_AF));
            }

            let remaining_timeout =
                deadline.map(|end| end.saturating_duration_since(std::time::Instant::now()));
            if let Some(remaining) = remaining_timeout
                && remaining.is_zero()
            {
                return Ok(None);
            }

            let until_wake = if state.alarm_enabled {
                state.alarm_wake_time.and_then(|wake_time| {
                    wake_time
                        .signed_duration_since(now)
                        .to_std()
                        .ok()
                        .filter(|d| !d.is_zero())
                })
            } else {
                None
            };

            let wait_duration = match (remaining_timeout, until_wake) {
                (Some(timeout), Some(until_wake)) => Some(timeout.min(until_wake)),
                (Some(timeout), None) => Some(timeout),
                (None, Some(until_wake)) => Some(until_wake),
                (None, None) => None,
            };

            match wait_duration {
                Some(duration) => {
                    let (guard, wait_result) = self
                        .cond
                        .wait_timeout(state, duration)
                        .map_err(|e| anyhow::anyhow!("condvar wait poisoned: {}", e))?;
                    state = guard;
                    if wait_result.timed_out() {
                        continue;
                    }
                }
                None => {
                    state = self
                        .cond
                        .wait(state)
                        .map_err(|e| anyhow::anyhow!("condvar wait poisoned: {}", e))?;
                }
            }
        }
    }
}
