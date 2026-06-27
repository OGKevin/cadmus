use anyhow::Error;
use chrono::{DateTime, Utc};
use sntpc::{NtpContext, StdTimestampGen};
use sntpc_net_std::UdpSocketWrapper;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::device::rtc::Rtc;
use crate::geolocation;
use crate::geolocation::GeoLocation;
use crate::http::Client as HttpClient;
use crate::view::{Event, NotificationEvent};

use std::sync::Arc;

const NTP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct TimeManager<R: Rtc> {
    rtc: Arc<R>,
    set_timezone_fn: fn(chrono_tz::Tz) -> Result<(), Error>,
}

impl<R: Rtc> TimeManager<R> {
    pub fn new(rtc: Arc<R>, set_timezone_fn: fn(chrono_tz::Tz) -> Result<(), Error>) -> Self {
        TimeManager {
            rtc,
            set_timezone_fn,
        }
    }

    pub fn sync(
        &self,
        ntp_host: &str,
        manual: bool,
        geolocation: Option<GeoLocation>,
        hub: &Sender<Event>,
    ) -> Result<(), Error> {
        if let Err(e) = self.detect_and_set_timezone(geolocation) {
            if manual {
                hub.send(Event::Notification(NotificationEvent::Show(crate::fl!(
                    "notification-timezone-detection-failed"
                ))))
                .ok();
            }
            tracing::warn!(error = %e, "timezone detection failed");
        }

        let ntp_time = match self.query_ntp(ntp_host) {
            Ok(t) => t,
            Err(e) => {
                if manual {
                    hub.send(Event::Notification(NotificationEvent::Show(crate::fl!(
                        "notification-time-sync-failed"
                    ))))
                    .ok();
                } else {
                    tracing::warn!(error = %e, "ntp query failed");
                }
                return Err(e);
            }
        };

        let result = self
            .set_system_clock(ntp_time)
            .and_then(|()| self.rtc.set_time(ntp_time));

        match result {
            Ok(()) => {
                tracing::info!(time = %ntp_time, "time synced");
                hub.send(Event::ClockTick).ok();
                Ok(())
            }
            Err(e) => {
                if manual {
                    hub.send(Event::Notification(NotificationEvent::Show(crate::fl!(
                        "notification-time-sync-failed"
                    ))))
                    .ok();
                }
                tracing::warn!(error = %e, "set_system_clock or rtc.set_time failed");
                Err(e)
            }
        }
    }

    fn detect_and_set_timezone(&self, geolocation: Option<GeoLocation>) -> Result<(), Error> {
        let geo = match geolocation {
            Some(geo) => geo,
            None => {
                let client = HttpClient::new()?;

                geolocation::fetch_geolocation(&client)?
            }
        };

        (self.set_timezone_fn)(geo.timezone)?;

        Ok(())
    }

    fn query_ntp(&self, host: &str) -> Result<DateTime<Utc>, Error> {
        query_ntp(host)
    }

    fn set_system_clock(&self, time: DateTime<Utc>) -> Result<(), Error> {
        let tv = libc::timeval {
            tv_sec: time.timestamp() as libc::time_t,
            tv_usec: time.timestamp_subsec_micros() as libc::suseconds_t,
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

fn query_ntp(host: &str) -> Result<DateTime<Utc>, Error> {
    let addrs: Vec<_> = host.to_socket_addrs()?.collect();

    let mut last_err = None;
    for addr in &addrs {
        let bind_addr = match addr {
            std::net::SocketAddr::V4(_) => "0.0.0.0:0",
            std::net::SocketAddr::V6(_) => "[::]:0",
        };

        let socket = match UdpSocket::bind(bind_addr) {
            Ok(s) => s,
            Err(e) => {
                last_err = Some(anyhow::anyhow!("UDP bind failed for {bind_addr}: {e}"));
                continue;
            }
        };

        if socket.set_read_timeout(Some(NTP_TIMEOUT)).is_err() {
            continue;
        }

        let socket = UdpSocketWrapper::new(socket);
        let context = NtpContext::new(StdTimestampGen::default());

        match sntpc::sync::get_time(*addr, &socket, context) {
            Ok(result) => {
                let now = Utc::now();
                let offset = chrono::Duration::microseconds(result.offset());
                return Ok(now + offset);
            }
            Err(e) => {
                last_err = Some(anyhow::anyhow!("NTP error: {e:?}"));
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("DNS resolution failed for NTP host: {host}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ignore]
    #[test]
    fn ntp_query_with_hostname() {
        let result = query_ntp("time.cloudflare.com:123");
        assert!(result.is_ok(), "NTP query failed: {:?}", result.err());

        let ntp_time = result.unwrap();
        let now = Utc::now();
        let diff = (now - ntp_time).num_seconds().abs();
        assert!(diff < 60, "NTP time off by {diff}s, expected <60s");
    }
}
