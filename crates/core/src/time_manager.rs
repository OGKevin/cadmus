use anyhow::Error;
use chrono::{DateTime, Utc};
use sntpc::{NtpContext, StdTimestampGen};
use sntpc_net_std::UdpSocketWrapper;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::device::CURRENT_DEVICE;
use crate::http::Client as HttpClient;
use crate::rtc::Rtc;
use crate::view::{Event, NotificationEvent};

const NTP_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TimeManager {
    rtc: Rtc,
}

impl TimeManager {
    pub fn new(rtc: Rtc) -> Self {
        TimeManager { rtc }
    }

    pub fn sync(&self, ntp_host: &str, manual: bool, hub: &Sender<Event>) -> Result<(), Error> {
        if let Err(e) = self.detect_and_set_timezone() {
            if manual {
                hub.send(Event::Notification(NotificationEvent::Show(e.to_string())))
                    .ok();
            } else {
                tracing::warn!(error = %e, "timezone detection failed");
            }
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

        self.set_system_clock(ntp_time)?;
        self.rtc.set_time(ntp_time)?;

        tracing::info!(time = %ntp_time, "time synced");
        hub.send(Event::ClockTick).ok();

        Ok(())
    }

    fn detect_and_set_timezone(&self) -> Result<chrono_tz::Tz, Error> {
        let client = HttpClient::new()?;
        let resp: serde_json::Value = client
            .get("https://ipapi.co/json/")
            .timeout(Duration::from_secs(10))
            .send()?
            .json()?;

        let tz = resp["timezone"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("timezone field missing from ipapi response"))?
            .parse::<chrono_tz::Tz>()
            .map_err(|e| anyhow::anyhow!("invalid timezone from ipapi: {e}"))?;

        CURRENT_DEVICE.set_system_timezone(tz)?;
        Ok(tz)
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
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(NTP_TIMEOUT))?;
    let socket = UdpSocketWrapper::new(socket);
    let context = NtpContext::new(StdTimestampGen::default());

    let addr = host
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("DNS resolution failed for NTP host: {host}"))?;

    let result = sntpc::sync::get_time(addr, &socket, context)
        .map_err(|e| anyhow::anyhow!("NTP error: {:?}", e))?;

    let now = Utc::now();
    let offset = chrono::Duration::microseconds(result.offset());
    Ok(now + offset)
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
