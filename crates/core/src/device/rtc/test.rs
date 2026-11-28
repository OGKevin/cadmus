//! In-memory RTC for unit tests with assertion helpers.

use anyhow::Error;
use chrono::{DateTime, Days, Duration as ChronoDuration, Timelike, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::manager::Rtc;
use super::{RtcTime, RtcWkalrm};

const RTC_AF: u32 = 0x20;

#[derive(Debug)]
struct TestRtcState {
    /// Emulated hardware clock instant used by [`Rtc::read_time`] and due checks.
    current_time: DateTime<Utc>,
    /// Whether the in-memory hardware wake alarm is armed.
    alarm_enabled: bool,
    /// Programmed wake instant on the test RTC timeline, if any.
    alarm_wake_time: Option<DateTime<Utc>>,
    /// Instant the emulated hardware raises its alarm IRQ.
    ///
    /// Equal to [`Self::alarm_wake_time`] unless [`Self::time_of_day_alarms`] is
    /// set, in which case the hardware fires at the next matching time of day.
    alarm_fire_time: Option<DateTime<Utc>>,
    /// Emulate hardware that compares only hour, minute and second.
    time_of_day_alarms: bool,
    /// Longest alarm span the emulated hardware can hold, when known.
    alarm_limit: Option<ChronoDuration>,
    /// Platform name the emulated RTC reports, when set.
    name: Option<Arc<str>>,
    /// Set when an IRQ should unblock [`Rtc::wait_for_alarm_irq`].
    irq_pending: bool,
    /// `RTC_now − system_now` from the last drift refresh (`new` / `set_time`).
    ///
    /// Positive means the test RTC reads ahead of civil time.
    drift: ChronoDuration,
    /// `new_time − old_time` from the latest [`Rtc::set_time`], if not yet taken.
    pending_step: Option<ChronoDuration>,
}

/// Assertable RTC test double for unit tests.
pub struct TestRtc {
    state: Arc<Mutex<TestRtcState>>,
    cond: Arc<Condvar>,
    fail_disable: Arc<AtomicBool>,
    fail_set_time: Arc<AtomicBool>,
    released: Arc<AtomicBool>,
}

impl Clone for TestRtc {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            cond: Arc::clone(&self.cond),
            fail_disable: Arc::clone(&self.fail_disable),
            fail_set_time: Arc::clone(&self.fail_set_time),
            released: Arc::clone(&self.released),
        }
    }
}

impl TestRtc {
    pub fn new() -> Self {
        let current_time = Utc::now();
        Self {
            state: Arc::new(Mutex::new(TestRtcState {
                current_time,
                alarm_enabled: false,
                alarm_wake_time: None,
                alarm_fire_time: None,
                time_of_day_alarms: false,
                alarm_limit: None,
                name: None,
                irq_pending: false,
                drift: current_time.signed_duration_since(Utc::now()),
                pending_step: None,
            })),
            cond: Arc::new(Condvar::new()),
            fail_disable: Arc::new(AtomicBool::new(false)),
            fail_set_time: Arc::new(AtomicBool::new(false)),
            released: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_fail_disable(&self, fail: bool) {
        self.fail_disable.store(fail, Ordering::Relaxed);
    }

    pub fn set_fail_set_time(&self, fail: bool) {
        self.fail_set_time.store(fail, Ordering::Relaxed);
    }

    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    pub fn scheduled_wake_time(&self) -> Option<DateTime<Utc>> {
        self.state.lock().ok().and_then(|s| s.alarm_wake_time)
    }

    /// Returns the instant the emulated hardware raises its alarm IRQ.
    ///
    /// Equal to [`TestRtc::scheduled_wake_time`] unless time-of-day alarms are
    /// enabled.
    pub fn hardware_fire_time(&self) -> Option<DateTime<Utc>> {
        self.state.lock().ok().and_then(|s| s.alarm_fire_time)
    }

    /// Emulates hardware that compares only hour, minute and second.
    ///
    /// Such an RTC fires at the next matching time of day, so an alarm more than
    /// a day ahead fires early unless [`TestRtc::set_alarm_limit`] caps it.
    pub fn set_time_of_day_alarms(&self, enabled: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.time_of_day_alarms = enabled;
        }
    }

    /// Sets the longest span the emulated hardware alarm can hold.
    pub fn set_alarm_limit(&self, limit: Option<ChronoDuration>) {
        if let Ok(mut state) = self.state.lock() {
            state.alarm_limit = limit;
        }
    }

    /// Sets the platform name the emulated RTC reports.
    pub fn set_name(&self, name: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.name = Some(Arc::from(name));
        }
    }

    pub fn alarm_enabled(&self) -> bool {
        self.state.lock().map(|s| s.alarm_enabled).unwrap_or(false)
    }

    pub fn simulate_alarm_fired(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.alarm_enabled = false;
            state.irq_pending = true;
            self.cond.notify_all();
        }
    }

    pub fn set_current_time(&self, time: DateTime<Utc>) {
        if let Ok(mut state) = self.state.lock() {
            state.current_time = time;
            self.cond.notify_all();
        }
    }
}

impl Default for TestRtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rtc for TestRtc {
    async fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        let wake_time = state
            .alarm_wake_time
            .map(RtcTime::from)
            .unwrap_or_else(|| RtcTime::from(state.current_time));
        Ok(RtcWkalrm {
            enabled: u8::from(state.alarm_enabled),
            pending: 0,
            time: wake_time,
        })
    }

    fn name(&self) -> Option<Arc<str>> {
        self.state.lock().ok().and_then(|state| state.name.clone())
    }

    fn alarm_limit(&self) -> Option<ChronoDuration> {
        self.state.lock().ok().and_then(|state| state.alarm_limit)
    }

    async fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = true;
        state.alarm_wake_time = Some(wake_time);
        state.alarm_fire_time = Some(if state.time_of_day_alarms {
            next_time_of_day_match(wake_time, state.current_time)
        } else {
            wake_time
        });
        self.cond.notify_all();
        Ok(0)
    }

    async fn disable_alarm(&self) -> Result<i32, Error> {
        if self.fail_disable.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("simulated disable_alarm failure"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = false;
        state.alarm_fire_time = None;
        self.cond.notify_all();
        Ok(0)
    }

    async fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        Ok(state.current_time)
    }

    async fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        if self.fail_set_time.load(Ordering::Relaxed) {
            return Err(anyhow::anyhow!("simulated set_time failure"));
        }
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        let old_time = state.current_time;
        state.current_time = time;
        state.drift = time.signed_duration_since(Utc::now());
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
        loop {
            if state.irq_pending {
                state.irq_pending = false;
                return Ok(Some(RTC_AF));
            }

            match timeout {
                Some(duration) => {
                    let (guard, timed_out) = self
                        .cond
                        .wait_timeout(state, duration)
                        .map_err(|e| anyhow::anyhow!("condvar wait poisoned: {}", e))?;
                    state = guard;
                    if timed_out.timed_out() && !state.irq_pending {
                        return Ok(None);
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

    fn release(&self) -> Result<(), Error> {
        self.released.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Returns the first instant at or after `now` whose time of day matches `requested`.
///
/// RTC hardware works at whole-second resolution, so the requested sub-second
/// part is dropped.
fn next_time_of_day_match(requested: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    let requested = requested.with_nanosecond(0).unwrap_or(requested);
    let time_of_day = requested.time();
    let today = now.date_naive().and_time(time_of_day);
    let today = DateTime::from_naive_utc_and_offset(today, Utc);
    if today > now {
        return today;
    }
    let tomorrow = (now.date_naive() + Days::new(1)).and_time(time_of_day);
    DateTime::from_naive_utc_and_offset(tomorrow, Utc)
}
