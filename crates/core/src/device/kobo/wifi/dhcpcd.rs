//! dhcpcd-dbus client for querying interface IP and current ESSID.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::device::wifi::{Essid, NetworkInfo, WifiError};

const DHCPCD_SERVICE: &str = "name.marples.roy.dhcpcd";
const DHCPCD_PATH: &str = "/name/marples/roy/dhcpcd";
const DHCPCD_INTERFACE: &str = "name.marples.roy.dhcpcd";

/// Reply / overall deadline for dhcpcd-dbus method calls.
const DHCPCD_METHOD_TIMEOUT: Duration = Duration::from_secs(5);

/// dhcpcd-dbus method: returns interface status maps including `IPAddress`.
const METHOD_GET_INTERFACES: &str = "GetInterfaces";

/// dhcpcd-dbus method: lists configured WPA networks for an interface.
///
/// Each row is `(id, ssid, bssid, flags)`; the associated network’s flags
/// contain [`NETWORK_FLAG_CURRENT`].
const METHOD_LIST_NETWORKS: &str = "ListNetworks";

/// Property key in a `GetInterfaces` status map for the leased IPv4 address
/// (host-endian `u32`).
const PROP_IP_ADDRESS: &str = "IPAddress";

/// Flag substring in a `ListNetworks` flags field for the associated network.
const NETWORK_FLAG_CURRENT: &str = "[CURRENT]";

/// Interface name → host-endian [`PROP_IP_ADDRESS`] u32 from [`METHOD_GET_INTERFACES`].
pub(crate) type InterfaceIpMap = HashMap<String, u32>;

/// One row from dhcpcd-dbus [`METHOD_LIST_NETWORKS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedNetwork {
    pub id: i32,
    pub ssid: String,
    pub bssid: String,
    pub flags: String,
}

/// Converts a dhcpcd `IPAddress` `u32` (native/host byte order) to [`Ipv4Addr`].
///
/// dhcpcd encodes the address from an `in_addr` on the device, so decode with
/// [`u32::to_ne_bytes`]. Kobo targets are little-endian; the sample below holds
/// on LE hosts only.
///
/// Device example: `2147592384` → `192.168.1.128`.
#[cfg_attr(feature = "tracing", tracing::instrument(ret(level = tracing::Level::TRACE)))]
pub(crate) fn ipv4_from_host_u32(host: u32) -> Ipv4Addr {
    Ipv4Addr::from(host.to_ne_bytes())
}

/// Returns the ESSID of the first network whose flags contain [`NETWORK_FLAG_CURRENT`].
#[cfg_attr(feature = "tracing", tracing::instrument(skip(networks), ret(level = tracing::Level::TRACE)))]
pub(crate) fn current_essid(networks: &[ListedNetwork]) -> Option<Essid> {
    networks
        .iter()
        .find(|n| n.flags.contains(NETWORK_FLAG_CURRENT))
        .map(|n| Essid::new(n.ssid.clone()))
}

/// Queries dhcpcd-dbus on one system-bus connection (list + get interfaces).
#[cfg_attr(feature = "tracing", tracing::instrument(fields(interface), ret))]
pub(crate) fn network_info_from_zbus(interface: &str) -> Result<Option<NetworkInfo>, WifiError> {
    block_on_with_timeout(network_info_zbus_async(interface))
}

fn assemble_network_info(
    interface: &str,
    essid: &Essid,
    ifaces: &InterfaceIpMap,
) -> Result<NetworkInfo, WifiError> {
    let Some(&host_ip) = ifaces.get(interface) else {
        tracing::warn!(
            interface,
            essid = %essid,
            "current network without IP address"
        );
        return Err(WifiError::Incomplete(format!(
            "interface {interface} has {NETWORK_FLAG_CURRENT} network but no {PROP_IP_ADDRESS}"
        )));
    };

    let ip = IpAddr::V4(ipv4_from_host_u32(host_ip));
    tracing::debug!(interface, ip = %ip, essid = %essid, "assembled network info");
    Ok(NetworkInfo {
        ip,
        essid: essid.clone(),
    })
}

fn block_on_with_timeout<F, T>(fut: F) -> Result<T, WifiError>
where
    F: std::future::Future<Output = Result<T, WifiError>>,
{
    crate::runtime::RUNTIME.block_on(async {
        tokio::time::timeout(DHCPCD_METHOD_TIMEOUT, fut)
            .await
            .map_err(|_| {
                WifiError::Dbus(format!(
                    "dhcpcd-dbus timed out after {}s",
                    DHCPCD_METHOD_TIMEOUT.as_secs()
                ))
            })?
    })
}

async fn dhcpcd_connection() -> Result<zbus::Connection, WifiError> {
    tracing::debug!("connecting to system bus for dhcpcd-dbus");
    zbus::connection::Builder::system()
        .map_err(|e| WifiError::Dbus(format!("system bus builder: {e}")))?
        .method_timeout(DHCPCD_METHOD_TIMEOUT)
        .build()
        .await
        .map_err(|e| WifiError::Dbus(format!("system bus connect: {e}")))
}

async fn dhcpcd_proxy<'a>(connection: &'a zbus::Connection) -> Result<zbus::Proxy<'a>, WifiError> {
    zbus::Proxy::new(connection, DHCPCD_SERVICE, DHCPCD_PATH, DHCPCD_INTERFACE)
        .await
        .map_err(|e| WifiError::Dbus(format!("dhcpcd proxy: {e}")))
}

async fn get_interfaces_with_proxy(proxy: &zbus::Proxy<'_>) -> Result<InterfaceIpMap, WifiError> {
    let raw: HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>> = proxy
        .call(METHOD_GET_INTERFACES, &())
        .await
        .map_err(|e| WifiError::Dbus(format!("{METHOD_GET_INTERFACES}: {e}")))?;

    let mut map = InterfaceIpMap::new();
    for (iface, props) in raw {
        if let Some(value) = props.get(PROP_IP_ADDRESS)
            && let Ok(host) = u32::try_from(value)
        {
            map.insert(iface, host);
        }
    }
    tracing::debug!(
        method = METHOD_GET_INTERFACES,
        count = map.len(),
        "parsed interface addresses"
    );
    Ok(map)
}

async fn list_networks_with_proxy(
    proxy: &zbus::Proxy<'_>,
    interface: &str,
) -> Result<Vec<ListedNetwork>, WifiError> {
    let rows: Vec<(i32, String, String, String)> = proxy
        .call(METHOD_LIST_NETWORKS, &(interface,))
        .await
        .map_err(|e| WifiError::Dbus(format!("{METHOD_LIST_NETWORKS}: {e}")))?;

    let networks = rows
        .into_iter()
        .map(|(id, ssid, bssid, flags)| ListedNetwork {
            id,
            ssid,
            bssid,
            flags,
        })
        .collect();
    tracing::debug!(method = METHOD_LIST_NETWORKS, interface, "succeeded");
    Ok(networks)
}

#[cfg_attr(feature = "tracing", tracing::instrument(fields(interface), ret))]
async fn network_info_zbus_async(interface: &str) -> Result<Option<NetworkInfo>, WifiError> {
    tracing::debug!(
        interface,
        "querying network info on one dhcpcd-dbus connection"
    );
    let connection = dhcpcd_connection().await?;
    let proxy = dhcpcd_proxy(&connection).await?;

    let networks = list_networks_with_proxy(&proxy, interface).await?;
    let Some(essid) = current_essid(&networks) else {
        tracing::debug!(interface, "no current network");
        return Ok(None);
    };

    let ifaces = get_interfaces_with_proxy(&proxy).await?;
    Ok(Some(assemble_network_info(interface, &essid, &ifaces)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn network(id: i32, ssid: &str, flags: &str) -> ListedNetwork {
        ListedNetwork {
            id,
            ssid: ssid.to_string(),
            bssid: "any".to_string(),
            flags: flags.to_string(),
        }
    }

    #[test]
    fn ipv4_from_host_u32_decodes_device_sample() {
        assert_eq!(
            ipv4_from_host_u32(2147592384),
            Ipv4Addr::new(192, 168, 1, 128)
        );
    }

    #[test]
    fn current_essid_finds_current_flag() {
        let networks = vec![
            network(0, "Guest", "[DISABLED]"),
            network(1, "Home", NETWORK_FLAG_CURRENT),
            network(2, "Other", ""),
        ];
        assert_eq!(
            current_essid(&networks).as_ref().map(Essid::as_str),
            Some("Home")
        );
    }

    #[test]
    fn current_essid_none_when_no_current() {
        let networks = vec![network(0, "Guest", "[DISABLED]")];
        assert!(current_essid(&networks).is_none());
    }

    #[test]
    fn current_essid_none_on_empty_list() {
        assert!(current_essid(&[]).is_none());
    }

    #[test]
    fn essid_display() {
        assert_eq!(Essid::new("Cafe").to_string(), "Cafe");
    }

    #[test]
    fn assemble_ok_when_ip_present() {
        let mut ifaces = InterfaceIpMap::new();
        ifaces.insert("wlan0".to_string(), 2147592384);
        let essid = Essid::new("Home");
        let info = assemble_network_info("wlan0", &essid, &ifaces).unwrap();
        assert_eq!(info.ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 128)));
        assert_eq!(info.essid.as_str(), "Home");
    }

    #[test]
    fn assemble_err_when_ip_missing() {
        let essid = Essid::new("Home");
        assert!(matches!(
            assemble_network_info("wlan0", &essid, &InterfaceIpMap::new()),
            Err(WifiError::Incomplete(_))
        ));
    }
}
