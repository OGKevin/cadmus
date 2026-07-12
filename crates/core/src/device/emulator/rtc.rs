use anyhow::Error;
use chrono::{DateTime, Utc};

use crate::device::rtc::{Rtc, RtcWkalrm};

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
