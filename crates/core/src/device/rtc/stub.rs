//! No-op RTC for emulator builds.

use anyhow::Error;
use chrono::{DateTime, Utc};

use super::RtcWkalrm;
use super::manager::Rtc;

/// Emulator RTC that performs no hardware operations.
#[derive(Clone, Copy, Default)]
pub struct NoopRtc;

impl Rtc for NoopRtc {
    fn alarm(&self) -> Result<RtcWkalrm, Error> {
        Ok(RtcWkalrm::default())
    }

    fn set_alarm(&self, _wake_time: DateTime<Utc>) -> Result<i32, Error> {
        Ok(0)
    }

    fn disable_alarm(&self) -> Result<i32, Error> {
        Ok(0)
    }

    fn read_time(&self) -> Result<DateTime<Utc>, Error> {
        Ok(Utc::now())
    }

    fn set_time(&self, _time: DateTime<Utc>) -> Result<(), Error> {
        Ok(())
    }
}
