//! WiFi mode and idle-disable event handling.

use crate::device::{AppContext, EventOutcome};
use crate::input::DeviceEvent;
use crate::settings::{WIFI_IDLE_TIMEOUT_MIN_MINUTES, WifiMode};
use crate::view::{EntryId, Event, Hub};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Spawns a background thread that periodically sends [`Event::MightDisableWifi`].
///
/// Polls every [`WIFI_IDLE_TIMEOUT_MIN_MINUTES`] (converted to a duration).
/// Positive idle timeouts below that minimum are clamped when settings are
/// loaded or edited. The wake channel allows an immediate check when the last
/// Auto-mode lease is released.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(hub, idle_wake),
        fields(wifi_idle_timeout, poll_interval_secs = tracing::field::Empty),
        level = tracing::Level::TRACE,
    )
)]
pub(super) fn spawn_wifi_idle_poller(
    hub: &Hub,
    wifi_idle_timeout: f32,
    idle_wake: mpsc::Receiver<()>,
) {
    if wifi_idle_timeout < 0.0 {
        tracing::debug!(wifi_idle_timeout, "wifi idle poller not started");
        return;
    }

    let hub = hub.clone();
    let poll_interval = Duration::from_secs_f32(WIFI_IDLE_TIMEOUT_MIN_MINUTES * 60.0);
    #[cfg(feature = "tracing")]
    tracing::Span::current().record("poll_interval_secs", poll_interval.as_secs_f32());
    tracing::info!(
        wifi_idle_timeout,
        poll_interval_secs = poll_interval.as_secs_f32(),
        "starting wifi idle poller"
    );

    if let Err(error) = thread::Builder::new()
        .name("wifi-idle-poll".into())
        .spawn(move || {
            tracing::debug!("wifi idle poller thread running");
            loop {
                match idle_wake.recv_timeout(poll_interval) {
                    Ok(()) => tracing::trace!("wifi idle wake"),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        tracing::trace!("wifi idle poll tick");
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        tracing::debug!("wifi idle poller stopping: wake channel closed");
                        break;
                    }
                }
                if hub.send((Event::MightDisableWifi).into()).is_err() {
                    tracing::debug!("wifi idle poller stopping: hub closed");
                    break;
                }
            }
        })
    {
        tracing::error!(error = %error, "failed to spawn wifi idle poller");
    }
}

/// Dispatches WiFi-related lifecycle events.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(event, hub, context),
        fields(event = ?event),
        ret(level = tracing::Level::TRACE),
        level = tracing::Level::TRACE,
    )
)]
pub(super) fn handle_event(event: &Event, hub: &Hub, context: &mut AppContext) -> EventOutcome {
    match event {
        Event::SetWifiMode(mode) => handle_set_wifi_mode(*mode, hub, context),
        Event::Select(EntryId::SetWifiMode(mode)) => handle_set_wifi_mode(*mode, hub, context),
        Event::MightDisableWifi => handle_might_disable_wifi(context),
        _ => EventOutcome::Unhandled,
    }
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(hub, context),
        fields(mode = %mode, previous = tracing::field::Empty),
        ret(level = tracing::Level::TRACE),
        level = tracing::Level::TRACE,
    )
)]
fn handle_set_wifi_mode(mode: WifiMode, hub: &Hub, context: &mut AppContext) -> EventOutcome {
    if context.settings.wifi == mode {
        tracing::trace!(mode = %mode, "wifi mode unchanged");
        return EventOutcome::Handled;
    }

    let previous = context.settings.wifi;
    #[cfg(feature = "tracing")]
    tracing::Span::current().record("previous", tracing::field::display(&previous));
    tracing::info!(previous = %previous, mode = %mode, "setting wifi mode");

    context.settings.wifi = mode;
    context.wifi_session.set_mode(mode);

    match mode {
        WifiMode::AlwaysOn => {
            let session = context.wifi_session.clone();
            let hub = hub.clone();
            thread::spawn(move || match session.enable_radio() {
                Ok(true) => {
                    hub.send((Event::Device(DeviceEvent::NetUp)).into()).ok();
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(error = %error, "Failed to enable WiFi");
                }
            });
        }
        WifiMode::Off => {
            let session = context.wifi_session.clone();
            thread::spawn(move || {
                if let Err(error) = session.disable_radio() {
                    tracing::error!(error = %error, "Failed to disable WiFi");
                }
            });
            context.online = false;
        }
        WifiMode::Auto => {
            if !context.wifi_session.has_holders() {
                let session = context.wifi_session.clone();
                thread::spawn(move || {
                    if let Err(error) = session.disable_radio() {
                        tracing::error!(error = %error, "Failed to disable WiFi for Auto mode");
                    }
                });
                context.online = false;
            } else {
                tracing::debug!(
                    holders = context.wifi_session.holders().len(),
                    "auto mode with active holders; leaving radio up"
                );
            }
        }
    }

    EventOutcome::Handled
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(context),
        fields(
            mode = tracing::field::Empty,
            holders = tracing::field::Empty,
            idle_secs = tracing::field::Empty,
            timeout_secs = tracing::field::Empty,
        ),
        ret(level = tracing::Level::TRACE),
        level = tracing::Level::TRACE,
    )
)]
fn handle_might_disable_wifi(context: &mut AppContext) -> EventOutcome {
    let mode = context.settings.wifi;
    #[cfg(feature = "tracing")]
    {
        tracing::Span::current().record("mode", tracing::field::display(&mode));
        tracing::Span::current().record("holders", context.wifi_session.holders().len());
    }

    if mode != WifiMode::Auto {
        tracing::trace!(mode = %mode, "might-disable skipped: not auto");
        return EventOutcome::Handled;
    }

    if context.wifi_session.has_holders() {
        tracing::trace!("might-disable skipped: active holders");
        return EventOutcome::Handled;
    }

    let Some(idle_since) = context.wifi_session.idle_since() else {
        tracing::trace!("might-disable skipped: idle not armed");
        return EventOutcome::Handled;
    };

    let timeout_secs = 60.0 * context.settings.wifi_idle_timeout;
    let elapsed = idle_since.elapsed();
    #[cfg(feature = "tracing")]
    {
        tracing::Span::current().record("idle_secs", elapsed.as_secs_f32());
        tracing::Span::current().record("timeout_secs", timeout_secs);
    }

    if elapsed < Duration::from_secs_f32(timeout_secs.max(0.0)) {
        tracing::trace!(
            idle_secs = elapsed.as_secs_f32(),
            timeout_secs,
            "might-disable skipped: idle deadline not reached"
        );
        return EventOutcome::Handled;
    }

    if !context.online && !context.wifi_session.wifi_manager().is_enabled() {
        tracing::debug!("might-disable: radio already down; clearing idle");
        context.wifi_session.clear_idle();
        return EventOutcome::Handled;
    }

    tracing::info!(
        elapsed_secs = elapsed.as_secs_f32(),
        timeout_secs,
        "Disabling WiFi after idle timeout"
    );

    context.wifi_session.mark_offline_pending();
    context.online = false;

    let session = context.wifi_session.clone();
    thread::spawn(move || {
        if let Err(error) = session.disable_radio() {
            tracing::error!(error = %error, "Failed to disable WiFi after idle");
        }
    });

    EventOutcome::Handled
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::test_harness::DeviceRuntimeHarness;
    use std::time::Duration;

    fn wait_for_wifi_thread() {
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn handle_set_wifi_mode_noop_on_duplicate() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.wifi = WifiMode::AlwaysOn;
        harness.context.wifi_session.set_mode(WifiMode::AlwaysOn);
        let outcome = handle_event(
            &Event::SetWifiMode(WifiMode::AlwaysOn),
            &harness.hub_tx,
            &mut harness.context,
        );
        assert_eq!(outcome, EventOutcome::Handled);
        wait_for_wifi_thread();
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .enable_call_count(),
            0
        );
    }

    #[test]
    fn handle_set_wifi_mode_enable_always_on() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.wifi = WifiMode::Off;
        let outcome = handle_event(
            &Event::SetWifiMode(WifiMode::AlwaysOn),
            &harness.hub_tx,
            &mut harness.context,
        );
        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(harness.context.settings.wifi, WifiMode::AlwaysOn);
        wait_for_wifi_thread();
        let wifi = harness.context.device.wifi_manager_for_test();
        assert_eq!(wifi.enable_call_count(), 1);
        assert_eq!(wifi.enabled(), Some(true));
    }

    #[test]
    fn handle_set_wifi_mode_disable_clears_online() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.wifi = WifiMode::AlwaysOn;
        harness.context.online = true;
        let outcome = handle_event(
            &Event::SetWifiMode(WifiMode::Off),
            &harness.hub_tx,
            &mut harness.context,
        );
        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(harness.context.settings.wifi, WifiMode::Off);
        assert!(!harness.context.online);
        wait_for_wifi_thread();
        let wifi = harness.context.device.wifi_manager_for_test();
        assert_eq!(wifi.disable_call_count(), 1);
    }

    #[test]
    fn handle_set_wifi_mode_always_on_sends_netup_when_connected() {
        use crate::device::wifi::{Essid, NetworkInfo};
        use crate::input::DeviceEvent;

        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.wifi = WifiMode::Off;
        harness
            .context
            .device
            .wifi_manager_for_test()
            .set_network_info(Ok(Some(NetworkInfo {
                ip: "192.168.1.1".parse().unwrap(),
                essid: Essid::new("test"),
            })));
        let outcome = handle_event(
            &Event::SetWifiMode(WifiMode::AlwaysOn),
            &harness.hub_tx,
            &mut harness.context,
        );
        assert_eq!(outcome, EventOutcome::Handled);
        wait_for_wifi_thread();
        let events = harness.drain_hub();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Device(DeviceEvent::NetUp))),
            "expected NetUp when already associated, got {events:?}"
        );
    }

    #[test]
    fn might_disable_wifi_after_idle() {
        let mut harness = DeviceRuntimeHarness::new();
        harness.context.settings.wifi = WifiMode::Auto;
        harness.context.settings.wifi_idle_timeout = 0.0;
        harness.context.wifi_session.set_mode(WifiMode::Auto);
        harness.context.online = true;
        harness.context.wifi_session.notify_online();
        let lease = harness.context.wifi_session.acquire("t").unwrap();
        drop(lease);
        assert!(harness.context.wifi_session.idle_since().is_some());

        let outcome = handle_event(
            &Event::MightDisableWifi,
            &harness.hub_tx,
            &mut harness.context,
        );
        assert_eq!(outcome, EventOutcome::Handled);
        wait_for_wifi_thread();
        assert!(!harness.context.online);
    }
}
