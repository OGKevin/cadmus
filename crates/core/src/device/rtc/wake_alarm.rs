//! Hardware RTC wake-alarm state and its comparison with the logical schedule.
//!
//! The RTC exposes one wake-alarm slot while [`AlarmManager`] multiplexes several
//! logical alarms onto it. Each [`Rtc`] implementation reports its own slot via
//! [`Rtc::wake_alarm`]; this module owns the value and the comparison against
//! what Cadmus asked for.
//!
//! The kernel exposes no writer provenance for that slot, so a mismatch is a
//! candidate for an unmanaged alarm, never proof of who wrote it.

#[cfg(any(feature = "kobo", docsrs, test))]
use super::AlarmType;
use chrono::{DateTime, Utc};
use std::fmt;

/// Contents of the single hardware RTC wake-alarm slot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HardwareWakeAlarm {
    /// No alarm is armed.
    Disabled,
    /// An alarm is armed for this instant.
    Armed(DateTime<Utc>),
    /// The slot could not be read, or this platform exposes no such slot.
    #[default]
    Unavailable,
}

impl fmt::Display for HardwareWakeAlarm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled"),
            Self::Armed(time) => write!(f, "{time}"),
            Self::Unavailable => f.write_str("unavailable"),
        }
    }
}

/// Alarm state captured immediately before deep-idle entry.
///
/// Only exists where suspend telemetry is compiled: `device::suspend` is gated on
/// the `kobo` feature, and the tests cover it on the host.
#[cfg(any(feature = "kobo", docsrs, test))]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct PreSleepAlarms {
    /// Hardware wake alarm programmed into the RTC.
    pub(crate) hardware: HardwareWakeAlarm,
    /// Earliest logical alarm held by [`AlarmManager`].
    pub(crate) cadmus: Option<(AlarmType, DateTime<Utc>)>,
}

#[cfg(any(feature = "kobo", docsrs, test))]
impl PreSleepAlarms {
    /// Whether the hardware slot disagrees with Cadmus's earliest logical alarm.
    ///
    /// The slot and the RTC driver both resolve to whole seconds, so the
    /// comparison truncates. An [`HardwareWakeAlarm::Unavailable`] slot never
    /// reports a mismatch because nothing was read to compare against.
    pub(crate) fn unmanaged_alarm_candidate(&self) -> bool {
        match (self.hardware, self.cadmus) {
            (HardwareWakeAlarm::Unavailable, _) => false,
            (HardwareWakeAlarm::Disabled, None) => false,
            (HardwareWakeAlarm::Disabled, Some(_)) | (HardwareWakeAlarm::Armed(_), None) => true,
            (HardwareWakeAlarm::Armed(hardware), Some((_, cadmus))) => {
                hardware.timestamp() != cadmus.timestamp()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed(epoch: i64) -> HardwareWakeAlarm {
        HardwareWakeAlarm::Armed(DateTime::from_timestamp(epoch, 0).unwrap())
    }

    #[test]
    fn matching_alarm_is_not_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: armed(1_700_000_000),
            cadmus: Some((
                AlarmType::CalendarUpdate,
                DateTime::from_timestamp(1_700_000_000, 250_000_000).unwrap(),
            )),
        };
        assert!(!pre.unmanaged_alarm_candidate());
    }

    #[test]
    fn differing_alarm_is_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: armed(1_700_000_000),
            cadmus: Some((
                AlarmType::CalendarUpdate,
                DateTime::from_timestamp(1_700_000_060, 0).unwrap(),
            )),
        };
        assert!(pre.unmanaged_alarm_candidate());
    }

    #[test]
    fn armed_hardware_without_cadmus_alarm_is_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: armed(1_700_000_000),
            cadmus: None,
        };
        assert!(pre.unmanaged_alarm_candidate());
    }

    #[test]
    fn disabled_hardware_with_cadmus_alarm_is_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: HardwareWakeAlarm::Disabled,
            cadmus: Some((
                AlarmType::AutoPowerOff,
                DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            )),
        };
        assert!(pre.unmanaged_alarm_candidate());
    }

    #[test]
    fn idle_hardware_without_cadmus_alarm_is_not_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: HardwareWakeAlarm::Disabled,
            cadmus: None,
        };
        assert!(!pre.unmanaged_alarm_candidate());
    }

    #[test]
    fn unavailable_hardware_is_never_a_candidate() {
        let pre = PreSleepAlarms {
            hardware: HardwareWakeAlarm::Unavailable,
            cadmus: Some((
                AlarmType::CalendarUpdate,
                DateTime::from_timestamp(1_700_000_060, 0).unwrap(),
            )),
        };
        assert!(!pre.unmanaged_alarm_candidate());
    }
}
