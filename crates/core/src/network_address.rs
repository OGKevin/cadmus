//! Network endpoint identity (hostname, IP, …).
//!
//! Currently string-backed only; structured IP variants can be added later
//! without changing call sites that take [`NetworkAddress`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

const NTP_CLOUDFLARE: &str = "time.cloudflare.com";

/// Network endpoint identity (hostname, IP, …).
///
/// Currently string-backed only; structured IP variants can be added later
/// without changing call sites that take `&NetworkAddress`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NetworkAddress(String);

impl Default for NetworkAddress {
    fn default() -> Self {
        Self::ntp_cloudflare()
    }
}

impl NetworkAddress {
    /// Cloudflare NTP hostname used as Cadmus's built-in time-sync server.
    pub fn ntp_cloudflare() -> Self {
        Self(NTP_CLOUDFLARE.to_string())
    }

    /// Returns the address as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn strip_trailing_ntp_port(s: &str) -> &str {
    if let Some(stripped) = s.strip_prefix('[').and_then(|rest| {
        rest.rsplit_once("]:")
            .filter(|(_, port)| *port == "123")
            .map(|(addr, _)| addr)
    }) {
        return stripped;
    }

    if let Some((host, port)) = s.rsplit_once(':') {
        if port == "123" && (!host.contains(':') || host.parse::<IpAddr>().is_ok()) {
            return host;
        }
    }

    s
}

impl AsRef<str> for NetworkAddress {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NetworkAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Error returned when a string cannot be parsed as a [`NetworkAddress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAddressParseError;

impl fmt::Display for NetworkAddressParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("network address must be a non-empty hostname or IP")
    }
}

impl std::error::Error for NetworkAddressParseError {}

impl FromStr for NetworkAddress {
    type Err = NetworkAddressParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(NetworkAddressParseError);
        }
        Ok(Self(strip_trailing_ntp_port(trimmed).to_string()))
    }
}

impl Serialize for NetworkAddress {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NetworkAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hostname() {
        let addr: NetworkAddress = "time.cloudflare.com".parse().unwrap();
        assert_eq!(addr.as_str(), "time.cloudflare.com");
    }

    #[test]
    fn parses_string_ip() {
        let addr: NetworkAddress = "192.168.1.1".parse().unwrap();
        assert_eq!(addr.as_str(), "192.168.1.1");
    }

    #[test]
    fn trims_whitespace() {
        let addr: NetworkAddress = "  pool.ntp.org  ".parse().unwrap();
        assert_eq!(addr.as_str(), "pool.ntp.org");
    }

    #[test]
    fn rejects_empty() {
        assert!("".parse::<NetworkAddress>().is_err());
        assert!("   ".parse::<NetworkAddress>().is_err());
    }

    #[test]
    fn ntp_cloudflare_host() {
        assert_eq!(
            NetworkAddress::ntp_cloudflare().as_str(),
            "time.cloudflare.com"
        );
    }

    #[test]
    fn serde_roundtrip() {
        let addr = NetworkAddress::ntp_cloudflare();
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(json, "\"time.cloudflare.com\"");
        let back: NetworkAddress = serde_json::from_str(&json).unwrap();
        assert_eq!(back, addr);
    }

    #[test]
    fn serde_rejects_empty() {
        assert!(serde_json::from_str::<NetworkAddress>("\"\"").is_err());
    }
}
