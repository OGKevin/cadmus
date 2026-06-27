//! In-memory RTC for unit tests with assertion helpers.

use anyhow::Error;
use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

use super::manager::Rtc;
use super::{RtcTime, RtcWkalrm};

#[derive(Debug)]
struct TestRtcState {
    current_time: DateTime<Utc>,
    alarm_enabled: bool,
    alarm_wake_time: Option<DateTime<Utc>>,
}

/// Assertable RTC test double for unit tests.
#[derive(Clone)]
pub struct TestRtc {
    state: Arc<Mutex<TestRtcState>>,
}

impl TestRtc {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(TestRtcState {
                current_time: Utc::now(),
                alarm_enabled: false,
                alarm_wake_time: None,
            })),
        }
    }

    pub fn scheduled_wake_time(&self) -> Option<DateTime<Utc>> {
        self.state.lock().ok().and_then(|s| s.alarm_wake_time)
    }

    pub fn alarm_enabled(&self) -> bool {
        self.state.lock().map(|s| s.alarm_enabled).unwrap_or(false)
    }

    pub fn simulate_alarm_fired(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.alarm_enabled = false;
        }
    }

    pub fn set_current_time(&self, time: DateTime<Utc>) {
        if let Ok(mut state) = self.state.lock() {
            state.current_time = time;
        }
    }
}

impl Default for TestRtc {
    fn default() -> Self {
        Self::new()
    }
}

impl Rtc for TestRtc {
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
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

    fn set_alarm(&self, wake_time: DateTime<Utc>) -> Result<i32, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = true;
        state.alarm_wake_time = Some(wake_time);
        Ok(0)
    }

    fn disable_alarm(&self) -> Result<i32, Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.alarm_enabled = false;
        Ok(0)
    }

    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        let state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        Ok(state.current_time)
    }

    fn set_time(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;
        state.current_time = time;
        Ok(())
    }
}
