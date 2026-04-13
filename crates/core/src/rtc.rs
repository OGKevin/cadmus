use anyhow::Error;
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use nix::{ioctl_none, ioctl_read, ioctl_write_ptr};
use std::collections::BTreeMap;
use std::fs::File;
use std::mem;
use std::os::unix::io::AsRawFd;
use std::path::Path;

ioctl_read!(rtc_read_alarm, b'p', 0x10, RtcWkalrm);
ioctl_write_ptr!(rtc_write_alarm, b'p', 0x0f, RtcWkalrm);
ioctl_none!(rtc_disable_alarm, b'p', 0x02);

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RtcTime {
    tm_sec: libc::c_int,
    tm_min: libc::c_int,
    tm_hour: libc::c_int,
    tm_mday: libc::c_int,
    tm_mon: libc::c_int,
    tm_year: libc::c_int,
    tm_wday: libc::c_int,
    tm_yday: libc::c_int,
    tm_isdst: libc::c_int,
}

impl Default for RtcWkalrm {
    fn default() -> Self {
        unsafe { mem::zeroed() }
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct RtcWkalrm {
    enabled: libc::c_uchar,
    pending: libc::c_uchar,
    time: RtcTime,
}

impl RtcTime {
    fn year(&self) -> i32 {
        1900 + self.tm_year as i32
    }
}

impl RtcWkalrm {
    pub fn enabled(&self) -> bool {
        self.enabled == 1
    }

    pub fn year(&self) -> i32 {
        self.time.year()
    }
}

pub struct Rtc(File);

impl Rtc {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Rtc, Error> {
        let file = File::open(path)?;
        Ok(Rtc(file))
    }

    pub fn alarm(&self) -> Result<RtcWkalrm, Error> {
        let mut rwa = RtcWkalrm::default();
        unsafe {
            rtc_read_alarm(self.0.as_raw_fd(), &mut rwa)
                .map(|_| rwa)
                .map_err(|e| e.into())
        }
    }

    pub fn set_alarm(&self, duration: Duration) -> Result<i32, Error> {
        let wt = Utc::now() + duration;
        let rwa = RtcWkalrm {
            enabled: 1,
            pending: 0,
            time: RtcTime {
                tm_sec: wt.second() as libc::c_int,
                tm_min: wt.minute() as libc::c_int,
                tm_hour: wt.hour() as libc::c_int,
                tm_mday: wt.day() as libc::c_int,
                tm_mon: wt.month0() as libc::c_int,
                tm_year: (wt.year() - 1900) as libc::c_int,
                tm_wday: -1,
                tm_yday: -1,
                tm_isdst: -1,
            },
        };
        unsafe { rtc_write_alarm(self.0.as_raw_fd(), &rwa).map_err(|e| e.into()) }
    }

    pub fn disable_alarm(&self) -> Result<i32, Error> {
        unsafe { rtc_disable_alarm(self.0.as_raw_fd()).map_err(|e| e.into()) }
    }
}

/// Identifies a logical alarm managed by [`AlarmManager`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlarmType {
    AutoPowerOff,
    CalendarUpdate,
}

pub struct ScheduledAlarm {
    pub alarm_type: AlarmType,
    pub wake_time: DateTime<Utc>,
}

/// Multiplexes multiple logical alarms onto a single hardware RTC alarm.
///
/// The hardware RTC supports only one wake alarm at a time. `AlarmManager`
/// maintains a map of logical alarms keyed by [`AlarmType`] and always
/// programs the hardware with the earliest upcoming wake time. After each
/// wake, [`check_fired_alarms`] determines which logical alarms fired and
/// reschedules the hardware for any remaining ones.
pub struct AlarmManager {
    rtc: Rtc,
    scheduled_alarms: BTreeMap<AlarmType, ScheduledAlarm>,
}

impl AlarmManager {
    pub fn new(rtc: Rtc) -> Self {
        AlarmManager {
            rtc,
            scheduled_alarms: BTreeMap::new(),
        }
    }

    /// Schedule a logical alarm to fire `duration` from now.
    ///
    /// If an alarm of the same type is already scheduled it is replaced.
    /// The hardware RTC is updated to reflect the new earliest wake time.
    pub fn schedule_alarm(
        &mut self,
        alarm_type: AlarmType,
        duration: Duration,
    ) -> Result<(), Error> {
        let wake_time = Utc::now() + duration;
        self.scheduled_alarms.insert(
            alarm_type,
            ScheduledAlarm {
                alarm_type,
                wake_time,
            },
        );
        self.update_hardware_alarm()?;
        Ok(())
    }

    /// Cancel a previously scheduled logical alarm.
    ///
    /// If no alarm of that type is scheduled this is a no-op. The hardware
    /// RTC is updated to reflect the new earliest remaining wake time.
    pub fn cancel_alarm(&mut self, alarm_type: AlarmType) -> Result<(), Error> {
        self.scheduled_alarms.remove(&alarm_type);
        self.update_hardware_alarm()?;
        Ok(())
    }

    /// Returns `true` if an alarm of `alarm_type` is scheduled for a future time.
    pub fn is_alarm_scheduled(&self, alarm_type: AlarmType) -> bool {
        self.scheduled_alarms
            .get(&alarm_type)
            .map(|alarm| alarm.wake_time > Utc::now())
            .unwrap_or(false)
    }

    /// Returns the number of seconds until `alarm_type` fires, or `None` if
    /// it is not scheduled.
    pub fn time_until_alarm(&self, alarm_type: AlarmType) -> Option<i64> {
        self.scheduled_alarms.get(&alarm_type).map(|alarm| {
            alarm
                .wake_time
                .signed_duration_since(Utc::now())
                .num_seconds()
        })
    }

    /// Determines which logical alarms fired during the last sleep cycle.
    ///
    /// `before` is the timestamp just before the device went to sleep and
    /// `after` is the timestamp just after it woke. A hardware alarm is
    /// considered fired when it is disabled or when the sleep duration is
    /// within 3 seconds of the expected wake time (accounting for RTC
    /// granularity). Any fired logical alarms are removed from the schedule
    /// and the hardware is reprogrammed for the next earliest alarm.
    pub fn check_fired_alarms(
        &mut self,
        after: DateTime<Utc>,
        before: DateTime<Utc>,
    ) -> Result<Vec<AlarmType>, Error> {
        let mut fired_types = Vec::new();

        if let Some((_, earliest_alarm)) = self
            .scheduled_alarms
            .iter()
            .min_by_key(|(_, alarm)| &alarm.wake_time)
        {
            let expected_duration = earliest_alarm.wake_time.signed_duration_since(before);

            let rwa = self.rtc.alarm()?;
            let hardware_alarm_fired = !rwa.enabled()
                || (rwa.year() <= 1970
                    && ((after - before) - expected_duration).num_seconds().abs() < 3);

            if hardware_alarm_fired {
                let now = Utc::now();
                let mut to_remove = Vec::new();

                for (alarm_type, scheduled_alarm) in &self.scheduled_alarms {
                    if (now - scheduled_alarm.wake_time).abs().num_milliseconds() <= 3000 {
                        fired_types.push(*alarm_type);
                        to_remove.push(*alarm_type);
                    }
                }

                for alarm_type in to_remove {
                    self.scheduled_alarms.remove(&alarm_type);
                }
            }
        }

        self.update_hardware_alarm()?;
        Ok(fired_types)
    }

    fn update_hardware_alarm(&self) -> Result<(), Error> {
        if let Some((_, earliest_alarm)) = self
            .scheduled_alarms
            .iter()
            .min_by_key(|(_, alarm)| &alarm.wake_time)
        {
            let duration = earliest_alarm.wake_time.signed_duration_since(Utc::now());

            if duration.num_seconds() > 0 {
                self.rtc.set_alarm(duration)?;
            } else {
                self.rtc.disable_alarm()?;
            }
        } else {
            self.rtc.disable_alarm()?;
        }
        Ok(())
    }
}
