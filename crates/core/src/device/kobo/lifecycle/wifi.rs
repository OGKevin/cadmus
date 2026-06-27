//! WiFi enable/disable event handling.

use crate::device::DeviceHardware as _;
use crate::device::wifi::WifiManager;
use crate::device::{AppContext, EventOutcome};
use crate::view::{EntryId, Event};
use std::thread;

/// Dispatches WiFi-related lifecycle events.
pub(super) fn handle_event(event: &Event, context: &mut AppContext) -> EventOutcome {
    match event {
        Event::SetWifi(enable) => handle_set_wifi(*enable, context),
        Event::Select(EntryId::ToggleWifi) => handle_set_wifi(!context.settings.wifi, context),
        _ => EventOutcome::Unhandled,
    }
}

/// Applies a WiFi on/off request from settings or the network UI.
///
/// No-ops when the requested state already matches `context.settings.wifi`.
/// Spawns a thread to enable or disable hardware WiFi and sets
/// `context.online = false` when disabling.
fn handle_set_wifi(enable: bool, context: &mut AppContext) -> EventOutcome {
    if context.settings.wifi == enable {
        return EventOutcome::Handled;
    }

    context.settings.wifi = enable;
    if let Ok(wifi) = context.device.wifi_manager() {
        if enable {
            thread::spawn(move || {
                if let Err(error) = wifi.enable() {
                    tracing::error!(error = %error, "Failed to enable WiFi");
                }
            });
        } else {
            thread::spawn(move || {
                if let Err(error) = wifi.disable() {
                    tracing::error!(error = %error, "Failed to disable WiFi");
                }
            });
            context.online = false;
        }
    }

    EventOutcome::Handled
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use std::time::Duration;

    fn wait_for_wifi_thread() {
        std::thread::sleep(Duration::from_millis(50));
    }

    #[test]
    fn handle_set_wifi_noop_on_duplicate() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.wifi = true;
        let outcome = handle_event(&Event::SetWifi(true), &mut harness.context);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.settings.wifi);
        wait_for_wifi_thread();
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .enable_call_count(),
            0
        );
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .disable_call_count(),
            0
        );
    }

    #[test]
    fn handle_set_wifi_enable_updates_settings() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.wifi = false;
        let outcome = handle_event(&Event::SetWifi(true), &mut harness.context);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.settings.wifi);
        wait_for_wifi_thread();
        let wifi = harness.context.device.wifi_manager_for_test();
        assert_eq!(wifi.enable_call_count(), 1);
        assert_eq!(wifi.enabled(), Some(true));
    }

    #[test]
    fn handle_set_wifi_disable_clears_online() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.wifi = true;
        harness.context.online = true;
        let outcome = handle_event(&Event::SetWifi(false), &mut harness.context);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!harness.context.settings.wifi);
        assert!(!harness.context.online);
        wait_for_wifi_thread();
        let wifi = harness.context.device.wifi_manager_for_test();
        assert_eq!(wifi.disable_call_count(), 1);
        assert!(wifi.was_disable_called());
        assert_eq!(wifi.enabled(), Some(false));
    }

    #[test]
    fn handle_event_toggle_wifi_delegates() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.wifi = false;
        let outcome = handle_event(&Event::Select(EntryId::ToggleWifi), &mut harness.context);
        assert_eq!(outcome, EventOutcome::Handled);
        assert!(harness.context.settings.wifi);
        wait_for_wifi_thread();
        assert_eq!(
            harness
                .context
                .device
                .wifi_manager_for_test()
                .enable_call_count(),
            1
        );
    }
}
