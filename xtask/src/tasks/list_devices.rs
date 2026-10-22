//! `cadmuscli list-devices` — list Cadmus devices on the LAN.
//!
//! Browses `_cadmus._udp.local.` (presence) and prints each hostname, addresses,
//! and TXT `version` / `kind` / `platform`.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

/// DNS-SD type Cadmus announces at boot (no application port).
const PRESENCE_TYPE: &str = "_cadmus._udp.local.";

/// Arguments for `cadmuscli list-devices`.
#[derive(Debug, Args)]
pub struct ListDevicesArgs {
    /// Seconds to wait for mDNS replies.
    #[arg(long, default_value_t = 2)]
    pub timeout: u64,
}

/// A Cadmus presence record resolved on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListedDevice {
    hostname: String,
    addresses: Vec<IpAddr>,
    version: Option<String>,
    kind: Option<String>,
    platform: Option<String>,
}

/// Browses for Cadmus presence records and prints them.
///
/// # Errors
///
/// Returns an error if the mDNS daemon cannot bind UDP 5353.
pub fn run(args: ListDevicesArgs) -> Result<()> {
    let devices = browse_devices(Duration::from_secs(args.timeout))?;
    print!("{}", format_devices(&devices));
    Ok(())
}

fn browse_devices(timeout: Duration) -> Result<Vec<ListedDevice>> {
    let daemon = ServiceDaemon::new().context("mDNS unavailable (UDP 5353)")?;
    let receiver = daemon
        .browse(PRESENCE_TYPE)
        .context("failed to browse Cadmus presence")?;
    let mut found = BTreeMap::new();
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(event) => apply_event(&mut found, event),
            Err(_) => break,
        }
    }
    let _ = daemon.stop_browse(PRESENCE_TYPE);
    let _ = daemon.shutdown();
    Ok(found.into_values().collect())
}

fn apply_event(found: &mut BTreeMap<String, ListedDevice>, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let fullname = info.get_fullname().to_string();
            found.insert(fullname, ListedDevice::from(&info));
        }
        ServiceEvent::ServiceRemoved(_ty, fullname) => {
            found.remove(&fullname);
        }
        _ => {}
    }
}

impl From<&ServiceInfo> for ListedDevice {
    fn from(info: &ServiceInfo) -> Self {
        let mut addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
        addresses.sort();
        Self {
            hostname: trim_dot(info.get_hostname()),
            addresses,
            version: info.get_property_val_str("version").map(str::to_string),
            kind: info.get_property_val_str("kind").map(str::to_string),
            platform: info.get_property_val_str("platform").map(str::to_string),
        }
    }
}

fn trim_dot(hostname: &str) -> String {
    hostname.trim_end_matches('.').to_string()
}

fn format_devices(devices: &[ListedDevice]) -> String {
    if devices.is_empty() {
        return "No Cadmus devices found on the network.\n".to_string();
    }
    let mut out = String::new();
    for device in devices {
        out.push_str(&device.hostname);
        out.push('\n');
        if device.addresses.is_empty() {
            out.push_str("  addresses: (none)\n");
        } else {
            let addrs: Vec<String> = device.addresses.iter().map(ToString::to_string).collect();
            out.push_str("  addresses: ");
            out.push_str(&addrs.join(", "));
            out.push('\n');
        }
        if let Some(version) = &device.version {
            out.push_str("  version: ");
            out.push_str(version);
            out.push('\n');
        }
        if let Some(kind) = &device.kind {
            out.push_str("  kind: ");
            out.push_str(kind);
            out.push('\n');
        }
        if let Some(platform) = &device.platform {
            out.push_str("  platform: ");
            out.push_str(platform);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn info(instance: &str, host: &str, addr: &str, txt: &[(&str, &str)]) -> ServiceInfo {
        let properties: HashMap<String, String> = txt
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        ServiceInfo::new(PRESENCE_TYPE, instance, host, addr, 0, properties)
            .expect("valid service info")
    }

    fn resolved(instance: &str, host: &str, addr: &str, txt: &[(&str, &str)]) -> ServiceEvent {
        ServiceEvent::ServiceResolved(info(instance, host, addr, txt))
    }

    #[test]
    fn format_empty_says_none_found() {
        assert_eq!(
            format_devices(&[]),
            "No Cadmus devices found on the network.\n"
        );
    }

    #[test]
    fn apply_event_keeps_txt_and_sorts_by_hostname() {
        let mut found = BTreeMap::new();
        apply_event(
            &mut found,
            resolved(
                "cadmus-z",
                "cadmus-z.local.",
                "10.0.0.2",
                &[
                    ("version", "v1"),
                    ("kind", "test"),
                    ("platform", "emulator"),
                ],
            ),
        );
        apply_event(
            &mut found,
            resolved(
                "cadmus-a",
                "cadmus-a.local.",
                "10.0.0.1",
                &[
                    ("version", "v2"),
                    ("kind", "standard"),
                    ("platform", "kobo"),
                ],
            ),
        );
        apply_event(
            &mut found,
            ServiceEvent::SearchStarted(PRESENCE_TYPE.into()),
        );
        let devices: Vec<_> = found.into_values().collect();
        assert_eq!(
            format_devices(&devices),
            "cadmus-a.local\n  addresses: 10.0.0.1\n  version: v2\n  kind: standard\n  platform: kobo\ncadmus-z.local\n  addresses: 10.0.0.2\n  version: v1\n  kind: test\n  platform: emulator\n"
        );
    }

    #[test]
    fn format_omits_platform_when_absent() {
        let mut found = BTreeMap::new();
        apply_event(
            &mut found,
            resolved(
                "cadmus-a",
                "cadmus-a.local.",
                "10.0.0.1",
                &[("version", "v1"), ("kind", "test")],
            ),
        );
        let devices: Vec<_> = found.into_values().collect();
        assert_eq!(
            format_devices(&devices),
            "cadmus-a.local\n  addresses: 10.0.0.1\n  version: v1\n  kind: test\n"
        );
    }

    #[test]
    fn later_resolve_replaces_the_same_hostname() {
        let mut found = BTreeMap::new();
        apply_event(
            &mut found,
            resolved("cadmus-a", "cadmus-a.local.", "10.0.0.1", &[]),
        );
        apply_event(
            &mut found,
            resolved(
                "cadmus-a",
                "cadmus-a.local.",
                "10.0.0.9",
                &[("version", "v9")],
            ),
        );
        let devices: Vec<_> = found.into_values().collect();
        assert_eq!(devices.len(), 1);
        assert_eq!(
            devices[0].addresses,
            ["10.0.0.9".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(devices[0].version.as_deref(), Some("v9"));
    }

    #[test]
    fn service_removed_drops_only_that_device() {
        let mut found = BTreeMap::new();
        let keep = info("cadmus-a", "cadmus-a.local.", "10.0.0.1", &[]);
        let gone = info("cadmus-z", "cadmus-z.local.", "10.0.0.2", &[]);
        let gone_name = gone.get_fullname().to_string();
        apply_event(&mut found, ServiceEvent::ServiceResolved(keep));
        apply_event(&mut found, ServiceEvent::ServiceResolved(gone));
        apply_event(
            &mut found,
            ServiceEvent::ServiceRemoved(PRESENCE_TYPE.into(), gone_name),
        );
        let devices: Vec<_> = found.into_values().collect();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].hostname, "cadmus-a.local");
    }
}
