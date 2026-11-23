//! WiFi status monitor using dhcpcd-dbus.
//!
//! Subscribes to signals from `name.marples.roy.dhcpcd` on the system bus and
//! sends a `NetUp` event when either `WpaStatus` reports `COMPLETED` with an IP
//! or an `Event` reports a newly acquired DHCP lease (`BOUND`, `REBOOT`).
//! Startup connectivity is reconciled by the device lifecycle (enable/disable +
//! `network_info`), not by probing dhcpcd here.

use std::collections::HashMap;
use std::num::NonZeroUsize;

use futures_util::stream::StreamExt;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "tracing")]
use opentelemetry::trace::Status;
#[cfg(feature = "tracing")]
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::input::DeviceEvent;
use crate::task::{BackgroundTask, TaskId};
use crate::view::Event;

const DHCPCCD_PATH: &str = "/name/marples/roy/dhcpcd";

const SIGNAL_QUEUE_CAPACITY: NonZeroUsize = NonZeroUsize::new(100).unwrap();

/// WiFi status monitor that listens for dhcpcd-dbus `WpaStatus` and `Event` signals.
pub struct WifiStatusMonitorTask;

#[async_trait::async_trait]
impl BackgroundTask for WifiStatusMonitorTask {
    fn id(&self) -> TaskId {
        TaskId::WifiStatusMonitor
    }

    async fn run(&mut self, hub: &crate::view::Hub, cancel: &CancellationToken) {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                tracing::info!("shutdown requested");
            }
            result = monitor(hub) => {
                if let Err(e) = result {
                    tracing::error!(error = %e, "wifi status monitor exited with error");
                }
            }
        }
    }
}

/// Matches every dhcpcd signal on [`DHCPCCD_PATH`]; [`process_signal`] routes by member.
fn dhcpcd_signal_rule() -> zbus::Result<zbus::OwnedMatchRule> {
    Ok(zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(DHCPCCD_PATH)
        .build()?
        .into())
}

async fn monitor(hub: &crate::view::Hub) -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::Connection::system().await?;
    tracing::info!("connected to system bus");

    let rule = dhcpcd_signal_rule()?;

    let mut stream =
        zbus::MessageStream::for_match_rule(rule, &connection, Some(SIGNAL_QUEUE_CAPACITY)).await?;

    tracing::info!(path = DHCPCCD_PATH, "subscribed to dhcpcd signals");

    loop {
        tokio::select! {
            biased;

            msg = stream.next() => {
                #[cfg(feature = "tracing")]
                let span = tracing::info_span!("wifi_status_monitor: received dbus message").entered();

                let Some(msg) = msg else { break };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        #[cfg(feature = "tracing")]
                        span.set_status(Status::error("failed to read dbus message"));
                        tracing::warn!(error = %e, "failed to read dbus message");
                        continue;
                    }
                };

                if let Err(e) = process_signal(&msg, hub) {
                    #[cfg(feature = "tracing")]
                    span.set_status(Status::error("failed to process dhcpcd signal"));
                    tracing::warn!(error = %e, "failed to process dhcpcd signal");
                }
            }
        }
    }

    tracing::info!("wifi status monitor stopped");
    Ok(())
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(msg, hub), ret(level=tracing::Level::TRACE)))]
fn process_wpa_status(
    msg: &zbus::Message,
    hub: &crate::view::Hub,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = msg.body();

    let interfaces: HashMap<String, HashMap<String, String>> = body.deserialize()?;

    check_interfaces(&interfaces, hub);

    Ok(())
}

/// Routes a dhcpcd signal to its handler by member name.
fn process_signal(
    msg: &zbus::Message,
    hub: &crate::view::Hub,
) -> Result<(), Box<dyn std::error::Error>> {
    match msg.header().member().map(|member| member.as_str()) {
        Some("WpaStatus") => process_wpa_status(msg, hub),
        Some("Event") => process_event(msg, hub),
        _ => Ok(()),
    }
}

/// Sends NetUp when a dhcpcd `Event` reports that the interface acquired a lease.
///
/// The body is the dhcpcd hook environment as `a{sv}`; `Reason` is the hook
/// reason as defined in [dhcpcd-run-hooks(8)]. Only `BOUND` (a new lease) and
/// `REBOOT` (a stored lease the server confirmed after reassociation) mark the
/// transition to online. `RENEW` and `REBIND` fire while a lease is already
/// held, so they are not a sign that Wi-Fi came up. `WpaStatus` does not
/// reliably follow a new lease, so the lease alone must bring the session
/// online.
///
/// [dhcpcd-run-hooks(8)]: https://github.com/NetworkConfiguration/dhcpcd/blob/master/hooks/dhcpcd-run-hooks.8.in
#[cfg_attr(feature = "tracing", tracing::instrument(skip(msg, hub), ret(level=tracing::Level::TRACE)))]
fn process_event(
    msg: &zbus::Message,
    hub: &crate::view::Hub,
) -> Result<(), Box<dyn std::error::Error>> {
    let body = msg.body();
    let config: HashMap<&str, zbus::Variant<'_>> = body.deserialize()?;
    let reason = config
        .get("Reason")
        .and_then(|reason| <&str>::try_from(reason).ok());

    tracing::debug!(reason, "dhcpcd event received");

    if matches!(reason, Some("BOUND" | "REBOOT")) {
        tracing::info!(reason, "network up detected via dhcpcd event");
        hub.send((Event::Device(DeviceEvent::NetUp)).into()).ok();
    }

    Ok(())
}

/// Checks WiFi interfaces for completed status and sends NetUp if detected.
///
/// This is a separate function to enable unit testing without requiring
/// a full zbus::Message.
fn check_interfaces(interfaces: &HashMap<String, HashMap<String, String>>, hub: &crate::view::Hub) {
    for (interface_name, properties) in interfaces {
        if let Some(wpa_state) = properties.get("wpa_state") {
            tracing::debug!(
                interface = %interface_name,
                status = %wpa_state,
                "WpaStatus received"
            );

            let has_ip = properties
                .get("ip_address")
                .is_some_and(|ip| !ip.is_empty());

            if wpa_state == "COMPLETED" && has_ip {
                tracing::info!("network up detected via dhcpcd-dbus");
                hub.send((Event::Device(DeviceEvent::NetUp)).into()).ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_interfaces_sends_netup_when_wpa_completed() {
        let (tx, mut rx) = crate::view::hub_channel();

        let mut interfaces = HashMap::new();
        let mut properties = HashMap::new();
        properties.insert("wpa_state".to_string(), "COMPLETED".to_string());
        properties.insert("ip_address".to_string(), "127.0.0.1".to_string());
        interfaces.insert("wlan0".to_string(), properties);

        check_interfaces(&interfaces, &tx);

        let event = rx.try_recv().expect("should receive NetUp event");
        assert!(matches!(event.event, Event::Device(DeviceEvent::NetUp)));
    }

    #[test]
    fn check_interfaces_handles_multiple_interfaces() {
        let (tx, _rx) = crate::view::hub_channel();

        let mut wlan0_props = HashMap::new();
        wlan0_props.insert("wpa_state".to_string(), "COMPLETED".to_string());
        wlan0_props.insert("ip_address".to_string(), "127.0.0.1".to_string());

        let mut wlan1_props = HashMap::new();
        wlan1_props.insert("wpa_state".to_string(), "SCANNING".to_string());
        wlan1_props.insert("ip_address".to_string(), "".to_string());

        let mut interfaces = HashMap::new();
        interfaces.insert("wlan0".to_string(), wlan0_props);
        interfaces.insert("wlan1".to_string(), wlan1_props);

        check_interfaces(&interfaces, &tx);
    }

    #[test]
    fn check_interfaces_does_not_send_netup_without_ip_address() {
        let (tx, mut rx) = crate::view::hub_channel();

        // WPA association complete but DHCP not yet negotiated — no ip_address
        let mut interfaces = HashMap::new();
        let mut properties = HashMap::new();
        properties.insert("wpa_state".to_string(), "COMPLETED".to_string());
        interfaces.insert("wlan0".to_string(), properties);

        check_interfaces(&interfaces, &tx);

        assert!(
            rx.try_recv().is_err(),
            "should not send NetUp before DHCP binding"
        );
    }

    #[test]
    fn check_interfaces_does_not_send_netup_with_empty_ip_address() {
        let (tx, mut rx) = crate::view::hub_channel();

        let mut interfaces = HashMap::new();
        let mut properties = HashMap::new();
        properties.insert("wpa_state".to_string(), "COMPLETED".to_string());
        properties.insert("ip_address".to_string(), "".to_string());
        interfaces.insert("wlan0".to_string(), properties);

        check_interfaces(&interfaces, &tx);

        assert!(
            rx.try_recv().is_err(),
            "should not send NetUp with empty ip_address"
        );
    }

    #[test]
    fn dhcpcd_signal_rule_matches_every_member_on_the_path() {
        let rule = dhcpcd_signal_rule().expect("rule should build").to_string();

        assert!(rule.contains("type='signal'"), "{rule}");
        assert!(rule.contains(&format!("path='{DHCPCCD_PATH}'")), "{rule}");
        assert!(!rule.contains("member="), "{rule}");
    }

    fn dhcpcd_event(reason: &str) -> zbus::Message {
        let config = HashMap::from([
            ("Interface", zbus::Variant::from("wlan0")),
            ("Reason", zbus::Variant::from(reason)),
            ("IPAddress", zbus::Variant::from(2_147_592_384_u32)),
        ]);
        zbus::Message::signal(DHCPCCD_PATH, "name.marples.roy.dhcpcd", "Event")
            .build(&config)
            .expect("signal should build")
    }

    #[test]
    fn new_lease_events_send_netup() {
        for reason in ["BOUND", "REBOOT"] {
            let (tx, mut rx) = crate::view::hub_channel();

            process_signal(&dhcpcd_event(reason), &tx).expect("event should decode");

            let event = rx.try_recv().expect("lease event should send NetUp");
            assert!(
                matches!(event.event, Event::Device(DeviceEvent::NetUp)),
                "{reason}"
            );
        }
    }

    #[test]
    fn events_without_a_new_lease_do_not_send_netup() {
        for reason in ["RENEW", "REBIND", "CARRIER", "NOCARRIER", "EXPIRE", "STOP"] {
            let (tx, mut rx) = crate::view::hub_channel();

            process_signal(&dhcpcd_event(reason), &tx).expect("event should decode");

            assert!(rx.try_recv().is_err(), "{reason} must not send NetUp");
        }
    }
}
