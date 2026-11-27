//! Process-wide mDNS responder.
//!
//! One [`mdns_sd::ServiceDaemon`] is started from the app and shut down
//! with the process. It always advertises Cadmus on `_cadmus._udp` with no
//! application port so other devices can resolve `cadmus.local`. Other
//! subsystems register extra service types
//! and drop the returned [`Registration`] to withdraw them.
//!
//! The hostname is a fixed `cadmus` label. `mdns-sd` resolves collisions on
//! its own, renaming the host to `cadmus-2.local` and so on; the session
//! follows those renames so later registrations and `GetStatus` report the
//! name the LAN actually sees.
//!
//! Records are announced on every interface, loopback included, so programs
//! on the device can browse without Wi-Fi. Each interface answers only with
//! the addresses valid on it, so answers on the LAN never carry `127.0.0.1`.
//!
//! The info-level announcement line comes from
//! [`mdns_sd::DaemonEvent::Announce`], once per interface and
//! re-announcement, so it means records actually went out. Accepting a
//! registration only logs `mdns registered` at debug.
//!
//! This module does not take a Wi-Fi lease. Multicast only leaves the radio
//! when some other holder already brought it up.

use crate::version::{BuildKind, get_current_version, get_version};
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PRESENCE_DNS_SD: &str = "_cadmus._udp.local.";
const CADMUS_PREFIX: &str = "cadmus-";
/// DNS label announced for this device. `mdns-sd` appends `-N` when another
/// host already answers to it.
const HOSTNAME: &str = "cadmus";
/// Highest service-name length `mdns-sd` accepts. The crate default is 15,
/// which rejects `cadmus-companion`. The measured name is `cadmus-<subsystem>`.
const SERVICE_NAME_LEN_MAX: u8 = 30;
#[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
const SUBSYSTEM_MAX_LEN: usize = SERVICE_NAME_LEN_MAX as usize - CADMUS_PREFIX.len();
/// Long enough for the daemon thread to notice `Exit`.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// DNS-SD subsystem label inside `_cadmus-<name>._tcp.local.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subsystem(String);

/// Why [`Subsystem::parse`] rejected a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
pub(crate) struct InvalidSubsystem;

impl fmt::Display for InvalidSubsystem {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, f), level = tracing::Level::TRACE))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid mDNS subsystem name")
    }
}

impl Subsystem {
    /// Parses a DNS-SD subsystem name: `a-z`, `0-9`, hyphen; at least one letter.
    #[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(crate) fn parse(name: &str) -> Result<Self, InvalidSubsystem> {
        if !valid_subsystem(name) {
            return Err(InvalidSubsystem);
        }
        Ok(Self(name.to_ascii_lowercase()))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn valid_subsystem(name: &str) -> bool {
    if name.is_empty() || name.len() > SUBSYSTEM_MAX_LEN {
        return false;
    }
    if name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return false;
    }
    let mut has_letter = false;
    for c in name.chars() {
        if c.is_ascii_alphabetic() {
            has_letter = true;
        } else if !c.is_ascii_digit() && c != '-' {
            return false;
        }
    }
    has_letter
}

/// DNS-SD type: presence, or `_cadmus-<subsystem>._tcp|udp.local.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ServiceType {
    /// Existence only: `_cadmus._udp.local.` with no listener.
    Presence,
    /// `_cadmus-<subsystem>._tcp.local.`
    #[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
    Tcp(Subsystem),
    /// `_cadmus-<subsystem>._udp.local.`
    #[cfg_attr(not(test), expect(dead_code, reason = "no UDP subsystem yet"))]
    Udp(Subsystem),
}

impl ServiceType {
    /// TCP type for a validated subsystem.
    #[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(crate) fn tcp(subsystem: Subsystem) -> Self {
        Self::Tcp(subsystem)
    }

    /// UDP type for a validated subsystem.
    #[cfg_attr(not(test), expect(dead_code, reason = "no UDP subsystem yet"))]
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(crate) fn udp(subsystem: Subsystem) -> Self {
        Self::Udp(subsystem)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn dns_sd(&self) -> String {
        match self {
            Self::Presence => PRESENCE_DNS_SD.to_string(),
            Self::Tcp(subsystem) => format!("_cadmus-{}._tcp.local.", subsystem.as_str()),
            Self::Udp(subsystem) => format!("_cadmus-{}._udp.local.", subsystem.as_str()),
        }
    }
}

impl fmt::Display for ServiceType {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, f), level = tracing::Level::TRACE))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dns_sd())
    }
}

/// Instance label for a DNS-SD record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceLabel(String);

impl InstanceLabel {
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller sets a label yet"))]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(label), level = tracing::Level::TRACE))]
    pub(crate) fn new(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which instance name to announce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Instance {
    /// Use the session hostname (`cadmus`, or the renamed label).
    Hostname,
    /// Use an explicit instance label.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller sets a label yet"))]
    Label(InstanceLabel),
}

impl Instance {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hostname), level = tracing::Level::TRACE))]
    fn resolve<'a>(&'a self, hostname: &'a str) -> &'a str {
        match self {
            Self::Hostname => hostname,
            Self::Label(label) => label.as_str(),
        }
    }
}

/// TXT key/value pairs on a DNS-SD record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TxtRecords(Vec<(String, String)>);

impl TxtRecords {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(pairs), level = tracing::Level::TRACE))]
    pub(crate) fn from_pairs(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(pairs.into_iter().collect())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn to_hashmap(&self) -> HashMap<String, String> {
        self.0.iter().cloned().collect()
    }
}

/// A service another subsystem wants announced.
pub(crate) struct Service {
    pub ty: ServiceType,
    pub instance: Instance,
    /// Application port. `0` means no listener (presence).
    pub port: u16,
    pub txt: TxtRecords,
    /// Preferred address. Loopback and unspecified are omitted from it;
    /// address auto still adds every address the host has, including
    /// `127.0.0.1` on `lo`.
    pub ip: Option<IpAddr>,
}

/// Handle that unregisters one service when dropped.
pub(crate) struct Registration {
    daemon: mdns_sd::ServiceDaemon,
    fullname: String,
}

impl Drop for Registration {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn drop(&mut self) {
        if let Err(error) = self.daemon.unregister(&self.fullname) {
            tracing::debug!(
                error = %error,
                fullname = %self.fullname,
                "mdns unregister failed"
            );
        }
    }
}

/// One mDNS daemon for the process, plus the default presence record.
pub struct MdnsSession {
    hostname: Arc<Mutex<String>>,
    daemon: Option<mdns_sd::ServiceDaemon>,
    presence: Option<Registration>,
}

impl MdnsSession {
    /// Builds a session that is not listening yet.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(crate) fn new() -> Self {
        Self {
            hostname: Arc::new(Mutex::new(HOSTNAME.to_string())),
            daemon: None,
            presence: None,
        }
    }

    /// The advertised hostname (`cadmus`, or `cadmus-2` after a rename),
    /// without `.local`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub(crate) fn hostname(&self) -> String {
        self.hostname
            .lock()
            .map(|host| host.clone())
            .unwrap_or_else(|_| HOSTNAME.to_string())
    }

    /// Starts the daemon and the presence record if they are not already up.
    ///
    /// The daemon skips interfaces it cannot bind. Only a failure to start it
    /// leaves the session inert; callers treat that as optional: HTTP
    /// listeners still work, LAN discovery does not.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub fn start(&mut self) {
        if self.daemon.is_some() {
            return;
        }
        let daemon = match mdns_sd::ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(error) => {
                tracing::warn!(error = %error, "mdns unavailable");
                return;
            }
        };
        if let Err(error) = daemon.set_service_name_len_max(SERVICE_NAME_LEN_MAX) {
            tracing::warn!(error = %error, "mdns service-name limit unchanged");
        }
        match daemon.monitor() {
            Ok(events) => monitor_events(events, Arc::clone(&self.hostname)),
            Err(error) => tracing::warn!(error = %error, "mdns event monitor unavailable"),
        }
        self.daemon = Some(daemon);
        let hostname = self.hostname();
        self.presence = self.register(Service {
            ty: ServiceType::Presence,
            instance: Instance::Hostname,
            port: 0,
            txt: presence_txt(),
            ip: None,
        });
        if self.presence.is_some() {
            tracing::debug!(hostname = %hostname, "mdns presence registered");
        }
    }

    /// Announces `service` until the returned guard is dropped.
    ///
    /// Returns [`None`] when the daemon is not running or the record is rejected.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, service), fields(ty = %service.ty, port = service.port), level = tracing::Level::TRACE)
    )]
    pub(crate) fn register(&self, service: Service) -> Option<Registration> {
        let daemon = self.daemon.as_ref()?;
        let hostname = self.hostname();
        let info = service_info(&hostname, &service)?;
        let fullname = info.get_fullname().to_string();
        if let Err(error) = daemon.register(info) {
            tracing::warn!(
                error = %error,
                ty = %service.ty,
                instance = service.instance.resolve(&hostname),
                "mdns register failed"
            );
            return None;
        }
        tracing::debug!(
            fullname = %fullname,
            port = service.port,
            "mdns registered"
        );
        Some(Registration {
            daemon: daemon.clone(),
            fullname,
        })
    }

    /// Withdraws presence and stops the daemon thread.
    ///
    /// Does not wait for the thread to exit. [`shutdown_and_wait`] does.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub fn shutdown(&mut self) {
        let _ = self.stop();
    }

    /// Withdraws presence, stops the daemon thread, and waits until it exits.
    ///
    /// Call this before tearing down interfaces so the goodbye leaves while
    /// they are still up.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub async fn shutdown_and_wait(&mut self) {
        let Some(receiver) = self.stop() else {
            return;
        };
        if tokio::time::timeout(SHUTDOWN_WAIT, receiver.into_recv_async())
            .await
            .is_err()
        {
            tracing::debug!("mdns shutdown timed out");
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn stop(&mut self) -> Option<mdns_sd::Receiver<mdns_sd::DaemonStatus>> {
        self.presence = None;
        let daemon = self.daemon.take()?;
        match daemon.shutdown() {
            Ok(receiver) => Some(receiver),
            Err(error) => {
                tracing::debug!(error = %error, "mdns shutdown failed");
                None
            }
        }
    }

    #[cfg(test)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub(crate) fn is_live(&self) -> bool {
        self.daemon.is_some()
    }

    #[cfg(test)]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub(crate) fn presence_registered(&self) -> bool {
        self.presence.is_some()
    }
}

impl Drop for MdnsSession {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What the monitor logs for one daemon event.
#[derive(Debug, PartialEq, Eq)]
enum MonitorLog {
    Announce { fullname: String, interface: String },
    Renamed { hostname: String },
    IpAdded(IpAddr),
    IpRemoved(IpAddr),
    Error(String),
}

/// Applies one daemon event and returns the line the monitor logs for it.
///
/// Kept free of the daemon and the runtime so the log contract is testable.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn handle_daemon_event(event: &mdns_sd::DaemonEvent, hostname: &mut String) -> Option<MonitorLog> {
    match event {
        mdns_sd::DaemonEvent::Announce(fullname, payload) => Some(MonitorLog::Announce {
            fullname: fullname.clone(),
            interface: announce_interface(payload).to_string(),
        }),
        mdns_sd::DaemonEvent::NameChange(change) => {
            let name = host_record_name(change)?;
            hostname.clone_from(&name);
            Some(MonitorLog::Renamed { hostname: name })
        }
        mdns_sd::DaemonEvent::IpAdd(ip) => Some(MonitorLog::IpAdded(*ip)),
        mdns_sd::DaemonEvent::IpDel(ip) => Some(MonitorLog::IpRemoved(*ip)),
        mdns_sd::DaemonEvent::Error(error) => Some(MonitorLog::Error(error.to_string())),
        _ => None,
    }
}

/// Logs daemon events and applies the hostname renames `mdns-sd` makes.
///
/// The monitor channel is bounded and the daemon drops events when it is
/// full, so a task drains it continuously instead of on demand. The task
/// ends when the daemon shuts down and closes the channel.
fn monitor_events(events: mdns_sd::Receiver<mdns_sd::DaemonEvent>, hostname: Arc<Mutex<String>>) {
    tokio::spawn(async move {
        while let Ok(event) = events.recv_async().await {
            let mut host = hostname
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match handle_daemon_event(&event, &mut host) {
                Some(MonitorLog::Announce {
                    fullname,
                    interface,
                }) => {
                    tracing::info!(fullname = %fullname, interface = %interface, "mdns announced");
                }
                Some(MonitorLog::Renamed { hostname }) => {
                    tracing::info!(hostname = %hostname, "mdns hostname renamed after a conflict");
                }
                Some(MonitorLog::IpAdded(ip)) => {
                    tracing::debug!(ip = %ip, "mdns interface address added");
                }
                Some(MonitorLog::IpRemoved(ip)) => {
                    tracing::debug!(ip = %ip, "mdns interface address removed");
                }
                Some(MonitorLog::Error(error)) => {
                    tracing::warn!(error = %error, "mdns daemon error");
                }
                None => {}
            }
        }
    });
}

/// Interface name from an [`mdns_sd::DaemonEvent::Announce`] payload.
///
/// Re-announcements carry `<host>:<interface>`. The first announcement after a
/// registration carries the debug list of the addresses that went out, which
/// names no interface, so it is logged as given.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn announce_interface(payload: &str) -> &str {
    match payload.rsplit_once(':') {
        Some((host, interface)) if host.ends_with(".local.") || host.ends_with(".local") => {
            interface
        }
        _ => payload,
    }
}

/// Host label from a rename `mdns-sd` made after a name conflict.
///
/// Only host records (`A` / `AAAA`) carry the hostname; instance renames come
/// through the same event with a different record type and are ignored.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn host_record_name(change: &mdns_sd::DnsNameChange) -> Option<String> {
    if !matches!(change.rr_type, mdns_sd::RRType::A | mdns_sd::RRType::AAAA) {
        return None;
    }
    let name = change
        .new_name
        .strip_suffix(".local.")
        .or_else(|| change.new_name.strip_suffix(".local"))?;
    Some(name.to_string())
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn advertised_addr(ip: Option<IpAddr>) -> String {
    match ip {
        Some(addr) if !addr.is_unspecified() && !addr.is_loopback() => addr.to_string(),
        _ => String::new(),
    }
}

/// Device type advertised in presence TXT under `platform`.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn platform() -> &'static str {
    if cfg!(feature = "kobo") {
        "kobo"
    } else if cfg!(feature = "emulator") {
        "emulator"
    } else {
        "deviceless"
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn presence_txt() -> TxtRecords {
    let kind = match get_version().build_kind() {
        BuildKind::Test => "test",
        BuildKind::Standard => "standard",
    };
    TxtRecords::from_pairs([
        ("version".into(), get_current_version().to_string()),
        ("kind".into(), kind.into()),
        ("platform".into(), platform().into()),
    ])
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(service), level = tracing::Level::TRACE))]
fn service_info(hostname: &str, service: &Service) -> Option<mdns_sd::ServiceInfo> {
    let instance = service.instance.resolve(hostname);
    let host = format!("{hostname}.local.");
    let advertised = advertised_addr(service.ip);
    let ty = service.ty.dns_sd();
    match mdns_sd::ServiceInfo::new(
        ty.as_str(),
        instance,
        &host,
        advertised.as_str(),
        service.port,
        service.txt.to_hashmap(),
    ) {
        Ok(info) => Some(info.enable_addr_auto()),
        Err(error) => {
            tracing::warn!(error = %error, ty = %service.ty, "mdns service rejected");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_subsystem(name: &str) -> Subsystem {
        Subsystem::parse(name).expect("valid test subsystem")
    }

    #[test]
    fn host_record_name_tracks_conflict_renames_only() {
        let change = |new_name: &str, rr_type: mdns_sd::RRType| mdns_sd::DnsNameChange {
            original: "cadmus".to_string(),
            new_name: new_name.to_string(),
            rr_type,
            intf_name: "en0".to_string(),
        };
        assert_eq!(
            host_record_name(&change("cadmus-2.local.", mdns_sd::RRType::A)),
            Some("cadmus-2".to_string())
        );
        assert_eq!(
            host_record_name(&change("cadmus-3.local.", mdns_sd::RRType::AAAA)),
            Some("cadmus-3".to_string())
        );
        assert_eq!(
            host_record_name(&change(
                "cadmus (2)._cadmus._udp.local.",
                mdns_sd::RRType::TXT
            )),
            None
        );
    }

    #[test]
    fn subsystem_parse_rejects_invalid_names() {
        assert!(Subsystem::parse("").is_err());
        assert!(Subsystem::parse("-x").is_err());
        assert!(Subsystem::parse("x-").is_err());
        assert!(Subsystem::parse("a--b").is_err());
        assert!(Subsystem::parse("123").is_err());
        assert!(Subsystem::parse("has space").is_err());
        assert_eq!(Subsystem::parse("Sync").unwrap().as_str(), "sync");
        assert!(Subsystem::parse("companion").is_ok());
        assert!(Subsystem::parse(&"a".repeat(SUBSYSTEM_MAX_LEN)).is_ok());
        assert!(Subsystem::parse(&"a".repeat(SUBSYSTEM_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn service_type_templates_subsystem_names() {
        assert_eq!(ServiceType::Presence.dns_sd(), "_cadmus._udp.local.");
        assert_eq!(
            ServiceType::tcp(must_subsystem("example")).dns_sd(),
            "_cadmus-example._tcp.local."
        );
        assert_eq!(
            ServiceType::udp(must_subsystem("sync")).dns_sd(),
            "_cadmus-sync._udp.local."
        );
    }

    #[test]
    fn instance_label_overrides_hostname() {
        assert_eq!(Instance::Hostname.resolve("cadmus"), "cadmus");
        assert_eq!(
            Instance::Label(InstanceLabel::new("other")).resolve("cadmus"),
            "other"
        );
    }

    #[test]
    fn presence_txt_includes_version_and_build_kind() {
        let map = presence_txt().to_hashmap();
        assert!(map.contains_key("version"));
        assert!(!map["version"].is_empty());
        assert!(
            map.get("kind")
                .is_some_and(|kind| kind == "test" || kind == "standard")
        );
        let expected_platform = if cfg!(feature = "kobo") {
            "kobo"
        } else if cfg!(feature = "emulator") {
            "emulator"
        } else {
            "deviceless"
        };
        assert_eq!(map["platform"], expected_platform);
    }

    #[test]
    fn register_on_inert_session_returns_none() {
        let session = MdnsSession::new();
        assert!(!session.is_live());
        assert!(
            session
                .register(Service {
                    ty: ServiceType::tcp(must_subsystem("example")),
                    instance: Instance::Hostname,
                    port: 80,
                    txt: TxtRecords::default(),
                    ip: None,
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn start_registers_presence_when_the_daemon_binds() {
        let mut session = MdnsSession::new();
        session.start();
        if !session.is_live() {
            return;
        }
        assert!(session.presence_registered());
        let tcp = session.register(Service {
            ty: ServiceType::tcp(must_subsystem("example")),
            instance: Instance::Hostname,
            port: 80,
            txt: TxtRecords::from_pairs([("path".into(), "/".into())]),
            ip: None,
        });
        assert!(tcp.is_some());
        drop(tcp);
        session.shutdown();
        assert!(!session.is_live());
    }

    #[test]
    fn daemon_events_produce_the_expected_logs() {
        let mut hostname = "cadmus".to_string();
        let announce = |payload: &str| {
            mdns_sd::DaemonEvent::Announce("_cadmus._udp.local.".to_string(), payload.to_string())
        };
        assert_eq!(
            handle_daemon_event(&announce("cadmus.local.:eth0"), &mut hostname),
            Some(MonitorLog::Announce {
                fullname: "_cadmus._udp.local.".to_string(),
                interface: "eth0".to_string(),
            })
        );
        assert_eq!(
            handle_daemon_event(&announce("[192.168.1.5, 127.0.0.1]"), &mut hostname),
            Some(MonitorLog::Announce {
                fullname: "_cadmus._udp.local.".to_string(),
                interface: "[192.168.1.5, 127.0.0.1]".to_string(),
            })
        );
        assert_eq!(hostname, "cadmus");

        let rename = |new_name: &str, rr_type| {
            mdns_sd::DaemonEvent::NameChange(mdns_sd::DnsNameChange {
                original: "cadmus".to_string(),
                new_name: new_name.to_string(),
                rr_type,
                intf_name: "en0".to_string(),
            })
        };
        assert_eq!(
            handle_daemon_event(
                &rename("cadmus-2.local.", mdns_sd::RRType::A),
                &mut hostname
            ),
            Some(MonitorLog::Renamed {
                hostname: "cadmus-2".to_string(),
            })
        );
        assert_eq!(hostname, "cadmus-2");
        assert_eq!(
            handle_daemon_event(
                &rename("cadmus (2)._cadmus._udp.local.", mdns_sd::RRType::TXT),
                &mut hostname
            ),
            None
        );
        assert_eq!(hostname, "cadmus-2");

        let ip: IpAddr = "192.168.1.5".parse().expect("valid test address");
        assert_eq!(
            handle_daemon_event(&mdns_sd::DaemonEvent::IpAdd(ip), &mut hostname),
            Some(MonitorLog::IpAdded(ip))
        );
        assert_eq!(
            handle_daemon_event(&mdns_sd::DaemonEvent::IpDel(ip), &mut hostname),
            Some(MonitorLog::IpRemoved(ip))
        );
        assert_eq!(
            handle_daemon_event(
                &mdns_sd::DaemonEvent::Error(mdns_sd::Error::Msg("boom".to_string())),
                &mut hostname
            ),
            Some(MonitorLog::Error("boom".to_string()))
        );
    }

    #[test]
    fn service_info_keeps_address_auto() {
        let service = |ip| Service {
            ty: ServiceType::Presence,
            instance: Instance::Hostname,
            port: 0,
            txt: TxtRecords::default(),
            ip,
        };
        let auto = service_info("cadmus", &service(None)).expect("valid presence info");
        assert!(auto.is_addr_auto());
        let preferred = service_info(
            "cadmus",
            &service(Some("192.168.1.5".parse().expect("valid test address"))),
        )
        .expect("valid presence info");
        assert!(preferred.is_addr_auto());
    }
}
