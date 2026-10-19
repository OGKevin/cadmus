//! Process-wide mDNS responder.
//!
//! One [`mdns_sd::ServiceDaemon`] is started from the app and shut down
//! with the process. It always advertises Cadmus on `_cadmus._udp` with no
//! application port so other devices can resolve `cadmus-<id>.local`. Other
//! subsystems register extra service types
//! and drop the returned [`Registration`] to withdraw them.
//!
//! This module does not take a Wi-Fi lease. Multicast only leaves the radio
//! when some other holder already brought it up.

use crate::version::{BuildKind, get_current_version, get_version};
use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

const PRESENCE_DNS_SD: &str = "_cadmus._udp.local.";
const CADMUS_PREFIX: &str = "cadmus-";
/// One DNS label, including the `cadmus-` prefix.
const HOSTNAME_LABEL_MAX: usize = 63;
/// Highest service-name length `mdns-sd` accepts. The crate default is 15,
/// which rejects `cadmus-companion`. The measured name is `cadmus-<subsystem>`.
const SERVICE_NAME_LEN_MAX: u8 = 30;
#[cfg_attr(not(test), expect(dead_code, reason = "no TCP caller yet"))]
const SUBSYSTEM_MAX_LEN: usize = SERVICE_NAME_LEN_MAX as usize - CADMUS_PREFIX.len();
/// Long enough for the daemon thread to notice `Exit`. The signal socket wakes it.
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
    /// Use the session hostname (`cadmus-<id>`).
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
    /// Preferred address. Loopback and unspecified are omitted; `enable_addr_auto`
    /// then fills addresses from the host.
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
    hostname: String,
    daemon: Option<mdns_sd::ServiceDaemon>,
    presence: Option<Registration>,
}

impl MdnsSession {
    /// Builds a session that is not listening yet.
    ///
    /// `serial` is the device serial when one exists. A device without one
    /// gets a random id that is stable for this process run, so two emulators
    /// on the same LAN do not claim the same hostname.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
    pub(crate) fn new(serial: &str) -> Self {
        Self {
            hostname: hostname_from_serial(serial),
            daemon: None,
            presence: None,
        }
    }

    /// The instance hostname (`cadmus-<id>`), without `.local`.
    #[expect(dead_code, reason = "no caller reads the hostname yet")]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), level = tracing::Level::TRACE))]
    pub(crate) fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Starts the daemon and the presence record if they are not already up.
    ///
    /// A bind failure on UDP 5353 leaves the session inert. Callers treat that
    /// as optional: HTTP listeners still work, LAN discovery does not.
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
        self.daemon = Some(daemon);
        self.presence = self.register(Service {
            ty: ServiceType::Presence,
            instance: Instance::Hostname,
            port: 0,
            txt: presence_txt(),
            ip: None,
        });
        if self.presence.is_some() {
            tracing::info!(hostname = %self.hostname, "mdns presence announced");
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
        let info = service_info(&self.hostname, &service)?;
        let fullname = info.get_fullname().to_string();
        if let Err(error) = daemon.register(info) {
            tracing::warn!(
                error = %error,
                ty = %service.ty,
                instance = service.instance.resolve(&self.hostname),
                "mdns register failed"
            );
            return None;
        }
        tracing::info!(
            fullname = %fullname,
            port = service.port,
            "mdns announced"
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

/// Stable mDNS instance from a device serial: `cadmus-<sanitized>`.
///
/// A serial that sanitizes to nothing yields `cadmus-<uuid>`, a random id
/// that is stable for this process run. Two serial-less devices on one LAN
/// therefore announce distinct hostnames.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
pub(crate) fn hostname_from_serial(serial: &str) -> String {
    let id = stable_id(serial).unwrap_or_else(generated_id);
    format!("{CADMUS_PREFIX}{id}")
}

/// Sanitized serial: alphanumerics lowercased, other runs folded to hyphens.
/// Capped so `cadmus-<id>` fits in one 63-byte DNS label. `None` when nothing
/// usable remains.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn stable_id(serial: &str) -> Option<String> {
    let limit = HOSTNAME_LABEL_MAX - CADMUS_PREFIX.len();
    let mut id = String::new();
    for c in serial.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
        if id.len() == limit {
            break;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() { None } else { Some(id) }
}

/// Random id for a device without a serial: 32 lowercase hex characters from a
/// v7 UUID, well under the 63-char DNS label limit.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
fn generated_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
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
    fn hostname_from_serial_sanitizes_non_empty_serials() {
        assert_eq!(hostname_from_serial("N000"), "cadmus-n000");
        assert_eq!(hostname_from_serial("AB CD"), "cadmus-ab-cd");
        assert_eq!(hostname_from_serial("ab--cd"), "cadmus-ab-cd");
    }

    #[test]
    fn hostname_from_serial_generates_random_ids_for_empty_serials() {
        let empty = hostname_from_serial("");
        let dashed = hostname_from_serial("---");
        for host in [&empty, &dashed] {
            let suffix = host.strip_prefix("cadmus-").expect("cadmus prefix");
            assert_eq!(suffix.len(), 32);
            assert!(suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        }
        assert_ne!(empty, hostname_from_serial(""));
        assert_ne!(dashed, hostname_from_serial("---"));
    }

    #[test]
    fn hostname_from_serial_caps_the_label_at_63() {
        let host = hostname_from_serial(&"A".repeat(80));
        assert!(host.starts_with(CADMUS_PREFIX));
        assert_eq!(host.len(), HOSTNAME_LABEL_MAX);
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
        assert_eq!(Instance::Hostname.resolve("cadmus-n000"), "cadmus-n000");
        assert_eq!(
            Instance::Label(InstanceLabel::new("other")).resolve("cadmus-n000"),
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
        let session = MdnsSession::new("N000");
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

    #[test]
    fn start_registers_presence_when_the_daemon_binds() {
        let mut session = MdnsSession::new("N000");
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
}
