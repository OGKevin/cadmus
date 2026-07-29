//! dhcpcd-dbus client for querying interface IP and current ESSID.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use crate::device::wifi::{Essid, NetworkInfo, WifiError};

const DHCPCCD_SERVICE: &str = "name.marples.roy.dhcpcd";
const DHCPCCD_PATH: &str = "/name/marples/roy/dhcpcd";
const DHCPCCD_INTERFACE: &str = "name.marples.roy.dhcpcd";

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

pub(crate) trait DhcpcdClient {
    fn get_interfaces(&self) -> Result<InterfaceIpMap, WifiError>;
    fn list_networks(&self, interface: &str) -> Result<Vec<ListedNetwork>, WifiError>;
}

/// Converts a host-endian dhcpcd `IPAddress` u32 to [`Ipv4Addr`].
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

/// Assembles [`NetworkInfo`] from a mockable dhcpcd client.
///
/// The caller must ensure Wi-Fi is enabled before invoking this; disabled
/// radio is [`WifiError::Disabled`] at the [`WifiManager`](crate::device::wifi::WifiManager)
/// boundary, not here.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(client), fields(interface), ret)
)]
pub(crate) fn network_info_with_client(
    interface: &str,
    client: &dyn DhcpcdClient,
) -> Result<Option<NetworkInfo>, WifiError> {
    tracing::debug!(interface, "listing networks via dhcpcd-dbus");
    let networks = client.list_networks(interface)?;
    let Some(essid) = current_essid(&networks) else {
        tracing::debug!(interface, "no current network");
        return Ok(None);
    };

    tracing::debug!(interface, essid = %essid, "fetching interfaces via dhcpcd-dbus");
    let ifaces = client.get_interfaces()?;
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
    Ok(Some(NetworkInfo { ip, essid }))
}

#[derive(Debug)]
pub(crate) struct ZbusDhcpcdClient;

impl DhcpcdClient for ZbusDhcpcdClient {
    #[cfg_attr(feature = "tracing", tracing::instrument(ret))]
    fn get_interfaces(&self) -> Result<InterfaceIpMap, WifiError> {
        crate::runtime::RUNTIME.block_on(get_interfaces_async())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(ret))]
    fn list_networks(&self, interface: &str) -> Result<Vec<ListedNetwork>, WifiError> {
        crate::runtime::RUNTIME.block_on(list_networks_async(interface))
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(ret))]
async fn get_interfaces_async() -> Result<InterfaceIpMap, WifiError> {
    tracing::debug!(method = METHOD_GET_INTERFACES, "connecting to system bus");
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| WifiError::Dbus(format!("system bus connect: {e}")))?;
    let proxy = zbus::Proxy::new(
        &connection,
        DHCPCCD_SERVICE,
        DHCPCCD_PATH,
        DHCPCCD_INTERFACE,
    )
    .await
    .map_err(|e| WifiError::Dbus(format!("dhcpcd proxy: {e}")))?;

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

#[cfg_attr(feature = "tracing", tracing::instrument(ret))]
async fn list_networks_async(interface: &str) -> Result<Vec<ListedNetwork>, WifiError> {
    tracing::debug!(
        method = METHOD_LIST_NETWORKS,
        interface,
        "connecting to system bus"
    );
    let connection = zbus::Connection::system()
        .await
        .map_err(|e| WifiError::Dbus(format!("system bus connect: {e}")))?;
    let proxy = zbus::Proxy::new(
        &connection,
        DHCPCCD_SERVICE,
        DHCPCCD_PATH,
        DHCPCCD_INTERFACE,
    )
    .await
    .map_err(|e| WifiError::Dbus(format!("dhcpcd proxy: {e}")))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::net::Ipv4Addr;

    struct MockDhcpcd {
        interfaces: Result<InterfaceIpMap, WifiError>,
        networks: Result<Vec<ListedNetwork>, WifiError>,
        list_calls: RefCell<u32>,
        get_calls: RefCell<u32>,
    }

    impl DhcpcdClient for MockDhcpcd {
        fn get_interfaces(&self) -> Result<InterfaceIpMap, WifiError> {
            *self.get_calls.borrow_mut() += 1;
            match &self.interfaces {
                Ok(m) => Ok(m.clone()),
                Err(e) => Err(clone_err(e)),
            }
        }

        fn list_networks(&self, _interface: &str) -> Result<Vec<ListedNetwork>, WifiError> {
            *self.list_calls.borrow_mut() += 1;
            match &self.networks {
                Ok(n) => Ok(n.clone()),
                Err(e) => Err(clone_err(e)),
            }
        }
    }

    fn clone_err(e: &WifiError) -> WifiError {
        match e {
            WifiError::Disabled => WifiError::Disabled,
            WifiError::Dbus(s) => WifiError::Dbus(s.clone()),
            WifiError::Incomplete(s) => WifiError::Incomplete(s.clone()),
            other => WifiError::Dbus(other.to_string()),
        }
    }

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
    fn assembly_ok_some_when_both_present() {
        let mut ifaces = InterfaceIpMap::new();
        ifaces.insert("wlan0".to_string(), 2147592384);
        let client = MockDhcpcd {
            interfaces: Ok(ifaces),
            networks: Ok(vec![network(0, "Home", NETWORK_FLAG_CURRENT)]),
            list_calls: RefCell::new(0),
            get_calls: RefCell::new(0),
        };
        let info = network_info_with_client("wlan0", &client)
            .unwrap()
            .expect("Some");
        assert_eq!(info.ip, IpAddr::V4(Ipv4Addr::new(192, 168, 1, 128)));
        assert_eq!(info.essid.as_str(), "Home");
        assert_eq!(*client.list_calls.borrow(), 1);
        assert_eq!(*client.get_calls.borrow(), 1);
    }

    #[test]
    fn assembly_ok_none_when_no_current() {
        let client = MockDhcpcd {
            interfaces: Ok(InterfaceIpMap::new()),
            networks: Ok(vec![network(0, "Guest", "[DISABLED]")]),
            list_calls: RefCell::new(0),
            get_calls: RefCell::new(0),
        };
        assert!(
            network_info_with_client("wlan0", &client)
                .unwrap()
                .is_none()
        );
        assert_eq!(*client.get_calls.borrow(), 0);
    }

    #[test]
    fn assembly_err_when_current_without_ip() {
        let client = MockDhcpcd {
            interfaces: Ok(InterfaceIpMap::new()),
            networks: Ok(vec![network(0, "Home", NETWORK_FLAG_CURRENT)]),
            list_calls: RefCell::new(0),
            get_calls: RefCell::new(0),
        };
        assert!(matches!(
            network_info_with_client("wlan0", &client),
            Err(WifiError::Incomplete(_))
        ));
    }

    #[test]
    fn assembly_err_on_dbus_failure() {
        let client = MockDhcpcd {
            interfaces: Ok(InterfaceIpMap::new()),
            networks: Err(WifiError::Dbus("boom".into())),
            list_calls: RefCell::new(0),
            get_calls: RefCell::new(0),
        };
        assert!(matches!(
            network_info_with_client("wlan0", &client),
            Err(WifiError::Dbus(_))
        ));
    }
}
