//! WiFi status monitor using dhcpcd-dbus.
//!
//! Subscribes to the `WpaStatus` signal from `name.marples.roy.dhcpcd` on the
//! system bus. When the status changes to `COMPLETED` with an IP, sends a
//! `NetUp` event. Startup connectivity is reconciled by the device lifecycle
//! (enable/disable + `network_info`), not by probing dhcpcd here.

use std::collections::HashMap;

use futures_util::stream::StreamExt;
use tokio_util::sync::CancellationToken;

#[cfg(feature = "tracing")]
use opentelemetry::trace::Status;
#[cfg(feature = "tracing")]
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::input::DeviceEvent;
use crate::task::{BackgroundTask, TaskFuture, TaskId};
use crate::view::Event;

const DHCPCCD_PATH: &str = "/name/marples/roy/dhcpcd";

/// WiFi status monitor that listens for dhcpcd-dbus WpaStatus signals.
pub struct WifiStatusMonitorTask;

impl BackgroundTask for WifiStatusMonitorTask {
    fn id(&self) -> TaskId {
        TaskId::WifiStatusMonitor
    }

    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
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
        })
    }
}

async fn monitor(hub: &crate::view::Hub) -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::Connection::system().await?;
    tracing::info!("connected to system bus");

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(DHCPCCD_PATH)?
        .member("WpaStatus")?
        .build();

    let mut stream = zbus::MessageStream::for_match_rule(rule, &connection, Some(100)).await?;

    tracing::info!(
        path = DHCPCCD_PATH,
        member = "WpaStatus",
        "subscribed to wifi status signals"
    );

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

                if let Err(e) = process_wpa_status(&msg, hub) {
                    #[cfg(feature = "tracing")]
                    span.set_status(Status::error("failed to process wpa status"));
                    tracing::warn!(error = %e, "failed to process wpa status");
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
    use std::sync::mpsc;

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
}
