use anyhow::Error;
use chrono::{DateTime, Duration, Utc};
use sntpc::{NtpContext, NtpUdpSocket, StdTimestampGen};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

use crate::device::rtc::{AlarmManager, Rtc};
use crate::geolocation;
use crate::geolocation::GeoLocation;
use crate::http::Client as HttpClient;
use crate::network_address::NetworkAddress;
use crate::view::{Event, NotificationEvent};

const NTP_PORT: u16 = 123;
const NTP_TIMEOUT: StdDuration = StdDuration::from_secs(5);

/// Clocks within this skew are treated as already in agreement.
pub(crate) const CLOCK_AGREEMENT_THRESHOLD: Duration = Duration::seconds(2);

/// Operating-system wall clock used by [`TimeManager`].
///
/// Distinct from the battery-backed [`Rtc`]: this is the kernel clock that
/// userspace and NTP see. Production uses `settimeofday`; tests inject a
/// mock so reconciliation can be exercised without changing the host clock.
///
/// Setting the clock is not atomic with a later RTC write. A failed hardware
/// update after a successful [`Self::set`] leaves the clocks on different
/// timelines and is recorded as [`ClockDivergence`].
trait SystemClock: Send + Sync {
    /// Returns the current system time in UTC.
    ///
    /// [`TimeManager::reconcile_at_startup`] compares this against the
    /// hardware clock to decide which timeline to trust after boot.
    fn now(&self) -> DateTime<Utc>;

    /// Sets the system clock to `time`.
    ///
    /// Applied before the matching RTC write so a failed hardware update can
    /// still be detected as divergence. May require elevated privileges.
    ///
    /// # Errors
    ///
    /// Returns an error when the kernel clock cannot be updated. In that
    /// case the hardware clock is left untouched.
    fn set(&self, time: DateTime<Utc>) -> Result<(), Error>;
}

struct OsSystemClock;

impl SystemClock for OsSystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn set(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let tv = libc::timeval {
            tv_sec: time.timestamp() as libc::time_t,
            tv_usec: i64::from(time.timestamp_subsec_micros()) as libc::suseconds_t,
        };
        let ret = unsafe { libc::settimeofday(&tv, std::ptr::null()) };
        if ret != 0 {
            return Err(anyhow::anyhow!(
                "settimeofday failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

/// Observed disagreement after the system clock was set but the hardware clock was not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockDivergence {
    pub system: DateTime<Utc>,
    pub hardware: DateTime<Utc>,
}

#[derive(Clone)]
pub struct TimeManager<R: Rtc> {
    rtc: Arc<R>,
    set_timezone_fn: fn(chrono_tz::Tz) -> Result<(), Error>,
    system_clock: Arc<dyn SystemClock>,
    last_divergence: Arc<Mutex<Option<ClockDivergence>>>,
}

impl<R: Rtc> TimeManager<R> {
    pub fn new(rtc: Arc<R>, set_timezone_fn: fn(chrono_tz::Tz) -> Result<(), Error>) -> Self {
        Self::with_system_clock(rtc, set_timezone_fn, Arc::new(OsSystemClock))
    }

    fn with_system_clock(
        rtc: Arc<R>,
        set_timezone_fn: fn(chrono_tz::Tz) -> Result<(), Error>,
        system_clock: Arc<dyn SystemClock>,
    ) -> Self {
        TimeManager {
            rtc,
            set_timezone_fn,
            system_clock,
            last_divergence: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn last_divergence(&self) -> Option<ClockDivergence> {
        self.last_divergence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .copied()
    }

    pub async fn sync(
        &self,
        ntp_server: &NetworkAddress,
        manual: bool,
        geolocation: Option<GeoLocation>,
        hub: &crate::view::Hub,
        alarm_manager: &Arc<Mutex<AlarmManager<R>>>,
    ) -> Result<(), Error> {
        let timezone_ok = match self.detect_and_set_timezone(geolocation).await {
            Ok(()) => true,
            Err(e) => {
                if manual {
                    hub.send(
                        (Event::Notification(NotificationEvent::Show(crate::fl!(
                            "notification-timezone-detection-failed"
                        ))))
                        .into(),
                    )
                    .ok();
                }
                tracing::warn!(error = %e, "timezone detection failed");
                false
            }
        };

        let ntp_time = match query_ntp(ntp_server).await {
            Ok(t) => t,
            Err(e) => {
                self.report_sync_failure(hub, manual, timezone_ok, &e);
                return Err(e);
            }
        };

        match self.apply_synchronised_time(ntp_time, alarm_manager) {
            Ok(()) => {
                tracing::info!(time = %ntp_time, address = %ntp_server, "time synced");
                hub.send((Event::ClockTick).into()).ok();
                Ok(())
            }
            Err(e) => {
                self.report_sync_failure(hub, manual, timezone_ok, &e);
                Err(e)
            }
        }
    }

    fn report_sync_failure(
        &self,
        hub: &crate::view::Hub,
        manual: bool,
        timezone_ok: bool,
        error: &Error,
    ) {
        let message = if timezone_ok {
            crate::fl!("notification-time-unsynchronised")
        } else {
            crate::fl!("notification-time-sync-failed")
        };
        if manual || timezone_ok {
            hub.send((Event::Notification(NotificationEvent::Show(message))).into())
                .ok();
        }
        tracing::warn!(error = %error, timezone_ok, "time synchronisation failed");
    }

    async fn detect_and_set_timezone(&self, geolocation: Option<GeoLocation>) -> Result<(), Error> {
        let geo = match geolocation {
            Some(geo) => geo,
            None => {
                let client = HttpClient::new()?;

                geolocation::fetch_geolocation(&client).await?
            }
        };

        (self.set_timezone_fn)(geo.timezone)?;

        Ok(())
    }

    fn apply_synchronised_time(
        &self,
        time: DateTime<Utc>,
        alarm_manager: &Arc<Mutex<AlarmManager<R>>>,
    ) -> Result<(), Error> {
        self.system_clock.set(time)?;
        let mut alarms = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
        self.write_hardware_clock(time)?;
        self.sync_alarms_after_clock_write(&mut alarms)
    }

    fn write_hardware_clock(&self, time: DateTime<Utc>) -> Result<(), Error> {
        match self.rtc.set_time(time) {
            Ok(()) => {
                self.set_last_divergence(None);
                Ok(())
            }
            Err(error) => {
                let hardware = self.rtc.read_time().ok();
                let divergence = ClockDivergence {
                    system: time,
                    hardware: hardware.unwrap_or(time),
                };
                self.set_last_divergence(Some(divergence));
                tracing::error!(
                    error = %error,
                    system = %divergence.system,
                    hardware = %divergence.hardware,
                    "hardware clock update failed after system clock was set"
                );
                Err(error)
            }
        }
    }

    fn sync_alarms_after_clock_write(&self, alarms: &mut AlarmManager<R>) -> Result<(), Error> {
        alarms.sync().inspect_err(|error| {
            tracing::error!(error = %error, "alarm rebase failed");
        })
    }

    fn set_last_divergence(&self, divergence: Option<ClockDivergence>) {
        *self
            .last_divergence
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = divergence;
    }

    /// Brings the system and hardware clocks into agreement at startup.
    ///
    /// Clocks within [`CLOCK_AGREEMENT_THRESHOLD`] are not rewritten, but
    /// [`AlarmManager::sync`] still runs so a leftover pending RTC step can
    /// rebase logical alarms. When the system clock is behind the hardware
    /// clock, this is treated as a cold boot and the hardware clock is
    /// applied. When the hardware clock is behind, the system clock is
    /// applied so a failed RTC write can be repaired. Mismatch paths go
    /// through [`Self::apply_synchronised_time`].
    pub fn reconcile_at_startup(
        &self,
        alarm_manager: &Arc<Mutex<AlarmManager<R>>>,
    ) -> Result<(), Error> {
        let system = self.system_clock.now();
        let hardware = self.rtc.read_time()?;
        let delta = hardware - system;

        if delta.abs() <= CLOCK_AGREEMENT_THRESHOLD {
            self.set_last_divergence(None);
            tracing::debug!(
                system = %system,
                hardware = %hardware,
                "clocks in agreement"
            );
            let mut alarms = alarm_manager.lock().unwrap_or_else(|e| e.into_inner());
            return self.sync_alarms_after_clock_write(&mut alarms);
        }

        let target = if delta > CLOCK_AGREEMENT_THRESHOLD {
            tracing::info!(
                system = %system,
                hardware = %hardware,
                "cold boot clock divergence; trusting hardware clock"
            );
            hardware
        } else {
            tracing::info!(
                system = %system,
                hardware = %hardware,
                "hardware clock behind system clock; applying system time"
            );
            system
        };

        self.apply_synchronised_time(target, alarm_manager)?;
        tracing::info!(target = %target, "startup clock reconciliation applied");
        Ok(())
    }
}

struct TokioNtpSocket(tokio::net::UdpSocket);

impl NtpUdpSocket for TokioNtpSocket {
    async fn send_to(&self, buf: &[u8], addr: SocketAddr) -> sntpc::Result<usize> {
        self.0
            .send_to(buf, addr)
            .await
            .map_err(|_| sntpc::Error::Network)
    }

    async fn recv_from(&self, buf: &mut [u8]) -> sntpc::Result<(usize, SocketAddr)> {
        self.0
            .recv_from(buf)
            .await
            .map_err(|_| sntpc::Error::Network)
    }
}

async fn query_ntp(server: &NetworkAddress) -> Result<DateTime<Utc>, Error> {
    let addrs: Vec<SocketAddr> = match server.as_str().parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, NTP_PORT)],
        Err(_) => match tokio::net::lookup_host((server.as_str(), NTP_PORT)).await {
            Ok(iter) => iter.collect(),
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "DNS resolution failed for NTP host: {server}: {error}"
                ));
            }
        },
    };

    let mut last_err = None;
    for addr in &addrs {
        let bind_addr = match addr {
            SocketAddr::V4(_) => "0.0.0.0:0",
            SocketAddr::V6(_) => "[::]:0",
        };

        let socket = match tokio::net::UdpSocket::bind(bind_addr).await {
            Ok(socket) => TokioNtpSocket(socket),
            Err(error) => {
                last_err = Some(anyhow::anyhow!("UDP bind failed for {bind_addr}: {error}"));
                continue;
            }
        };

        let context = NtpContext::new(StdTimestampGen::default());
        match tokio::time::timeout(NTP_TIMEOUT, sntpc::get_time(*addr, &socket, context)).await {
            Ok(Ok(result)) => {
                let now = Utc::now();
                let offset = chrono::Duration::microseconds(result.offset());
                return Ok(now + offset);
            }
            Ok(Err(error)) => {
                last_err = Some(anyhow::anyhow!("NTP error: {error:?}"));
            }
            Err(_elapsed) => {
                last_err = Some(anyhow::anyhow!("NTP query timed out"));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("DNS resolution failed for NTP host: {server}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::rtc::{AlarmManager, AlarmType, TestRtc};
    use chrono::TimeZone;

    struct MockSystemClock {
        now: Mutex<DateTime<Utc>>,
    }

    impl SystemClock for MockSystemClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn set(&self, time: DateTime<Utc>) -> Result<(), Error> {
            *self.now.lock().unwrap_or_else(|e| e.into_inner()) = time;
            Ok(())
        }
    }

    fn instant(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 6, 15, hour, 0, 0)
            .single()
            .expect("valid fixture instant")
    }

    type Fixture = (
        TimeManager<TestRtc>,
        Arc<TestRtc>,
        Arc<Mutex<AlarmManager<TestRtc>>>,
        Arc<MockSystemClock>,
    );

    fn fixture(system: DateTime<Utc>, hardware: DateTime<Utc>) -> Fixture {
        let rtc = TestRtc::new();
        rtc.set_current_time(hardware);
        let rtc = Arc::new(rtc);
        let system_clock = Arc::new(MockSystemClock {
            now: Mutex::new(system),
        });
        let time_manager = TimeManager::with_system_clock(
            Arc::clone(&rtc) as Arc<TestRtc>,
            |_| Ok(()),
            Arc::clone(&system_clock) as Arc<dyn SystemClock>,
        );
        let alarms = Arc::new(Mutex::new(AlarmManager::new(Arc::clone(&rtc))));
        (time_manager, rtc, alarms, system_clock)
    }

    #[test]
    fn reconcile_leaves_clocks_untouched_when_they_agree() {
        let system = instant(12);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, system);

        time_manager
            .reconcile_at_startup(&alarms)
            .expect("agreement should succeed");

        assert_eq!(system_clock.now(), system);
        assert_eq!(rtc.read_time().expect("rtc"), system);
        assert!(time_manager.last_divergence().is_none());
    }

    #[test]
    fn reconcile_trusts_hardware_when_system_is_behind() {
        let system = instant(8);
        let hardware = instant(12);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, hardware);

        time_manager
            .reconcile_at_startup(&alarms)
            .expect("cold boot reconcile should succeed");

        assert_eq!(system_clock.now(), hardware);
        assert_eq!(rtc.read_time().expect("rtc"), hardware);
        assert!(time_manager.last_divergence().is_none());
    }

    #[test]
    fn reconcile_applies_system_when_hardware_is_behind() {
        let system = instant(12);
        let hardware = instant(8);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, hardware);

        time_manager
            .reconcile_at_startup(&alarms)
            .expect("repair reconcile should succeed");

        assert_eq!(system_clock.now(), system);
        assert_eq!(rtc.read_time().expect("rtc"), system);
        assert!(time_manager.last_divergence().is_none());
    }

    #[test]
    fn apply_records_divergence_when_hardware_update_fails() {
        let system = instant(12);
        let hardware = instant(8);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, hardware);
        rtc.set_fail_set_time(true);

        let error = time_manager
            .apply_synchronised_time(system, &alarms)
            .expect_err("hardware failure should be reported");

        assert!(error.to_string().contains("simulated set_time failure"));
        assert_eq!(system_clock.now(), system);
        assert_eq!(rtc.read_time().expect("rtc"), hardware);
        assert_eq!(
            time_manager.last_divergence(),
            Some(ClockDivergence { system, hardware })
        );
    }

    #[test]
    fn apply_does_not_record_divergence_when_alarm_sync_fails() {
        let system = instant(12);
        let hardware = instant(8);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, hardware);
        rtc.set_fail_disable(true);

        let error = time_manager
            .apply_synchronised_time(system, &alarms)
            .expect_err("alarm sync failure should be reported");

        assert!(
            error
                .to_string()
                .contains("simulated disable_alarm failure")
        );
        assert_eq!(system_clock.now(), system);
        assert_eq!(rtc.read_time().expect("rtc"), system);
        assert!(time_manager.last_divergence().is_none());
    }

    #[test]
    fn reconcile_rebases_pending_step_when_clocks_agree() {
        let system = instant(12);
        let (time_manager, rtc, alarms, system_clock) = fixture(system, system);
        {
            let mut manager = alarms.lock().expect("alarms lock");
            manager
                .schedule_in(AlarmType::AutoSuspend, Duration::minutes(20))
                .expect("schedule");
        }
        let stepped = system + Duration::hours(1);
        rtc.set_time(stepped).expect("direct rtc write");
        system_clock.set(stepped).expect("system clock");

        time_manager
            .reconcile_at_startup(&alarms)
            .expect("agreement path should still sync alarms");

        let wake_after = rtc.scheduled_wake_time().expect("wake after");
        assert_eq!(
            wake_after.signed_duration_since(stepped),
            Duration::minutes(20)
        );
        assert!(time_manager.last_divergence().is_none());
    }

    #[test]
    fn reconcile_rebases_alarms_off_the_replaced_clock() {
        let system = instant(12);
        let hardware = instant(8);
        let (time_manager, rtc, alarms, _system_clock) = fixture(system, hardware);
        {
            let mut manager = alarms.lock().expect("alarms lock");
            manager
                .schedule_in(AlarmType::AutoSuspend, Duration::minutes(20))
                .expect("schedule");
        }
        let wake_before = rtc.scheduled_wake_time().expect("wake before");
        assert_eq!(
            wake_before.signed_duration_since(hardware),
            Duration::minutes(20)
        );

        time_manager
            .reconcile_at_startup(&alarms)
            .expect("reconcile");

        let wake_after = rtc.scheduled_wake_time().expect("wake after");
        assert_ne!(wake_after, wake_before);
        assert_eq!(
            wake_after.signed_duration_since(system),
            Duration::minutes(20)
        );
        assert_eq!(rtc.read_time().expect("rtc"), system);
    }

    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ntp_query_with_hostname() {
        let server = NetworkAddress::ntp_cloudflare();
        let result = crate::runtime::block_on(query_ntp(&server));
        assert!(result.is_ok(), "NTP query failed: {:?}", result.err());

        let ntp_time = result.unwrap();
        let now = Utc::now();
        let diff = (now - ntp_time).num_seconds().abs();
        assert!(diff < 60, "NTP time off by {diff}s, expected <60s");
    }
}
