//! Connected Wi-Fi network snapshot.

use std::fmt;
use std::net::IpAddr;

/// Connected Wi-Fi network name (ESSID / SSID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Essid(String);

impl Essid {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Essid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Snapshot of the active connection. Only constructed when the link is up
/// and both address and ESSID were obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub ip: IpAddr,
    pub essid: Essid,
}
