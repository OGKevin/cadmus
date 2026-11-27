//! API tests: listeners, bearer auth, settings round trips, and static files.

use super::public::{StartError, suspend, turn_off, turn_on};
use super::*;
use crate::device::DeviceHardware as _;
use crate::device::DevicePaths;
use crate::device::test_harness::DeviceRuntimeHarness;
use crate::device::wifi::{Essid, NetworkInfo};
use crate::document::file_extension::FileExtension;
use crate::fl;
use crate::i18n::DEFAULT_LOCALE;
use crate::i18n::I18nDisplay;
use crate::settings::WifiMode;
#[cfg(feature = "emulator")]
use crate::view::EntryId;
use cadmus_api_proto::cadmus::api::public::v1 as public_proto;
use connectrpc::ConnectError;
use secrecy::ExposeSecret;
use std::future::Future;
use std::time::Duration;
use tokio::sync::oneshot;

fn exposed_token(harness: &DeviceRuntimeHarness) -> Option<String> {
    harness
        .context
        .api
        .token
        .as_ref()
        .map(|token| token.expose_secret().to_string())
}

fn arm_wifi(harness: &mut DeviceRuntimeHarness, mode: WifiMode) {
    harness.context.settings.wifi = mode;
    harness.context.wifi_session.set_mode(mode);
    harness
        .context
        .device
        .wifi_manager_for_test()
        .set_network_info(Ok(Some(NetworkInfo {
            ip: "127.0.0.1".parse().unwrap(),
            essid: Essid::new("test"),
        })));
}

fn holds_public_api_lease(harness: &DeviceRuntimeHarness) -> bool {
    harness
        .context
        .wifi_session
        .holders()
        .iter()
        .any(|name| name.as_str() == LEASE_NAME)
}

async fn start(harness: &mut DeviceRuntimeHarness, addr: SocketAddr) -> Result<(), StartError> {
    crate::poll_parts!(harness, |hub, _bus, _rq, context, _runtime| {
        turn_on(hub, context, addr, true)
    })?;
    drive_listener_start(harness).await;
    Ok(())
}

/// Drain until the deferred lease-and-bind start has reported back.
async fn drive_listener_start(harness: &mut DeviceRuntimeHarness) {
    for _ in 0..100 {
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, rq, context, runtime)
        });
        if harness.context.api.pending_start.is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("listener start did not finish");
}

/// Poll the main loop until `request` finishes, bounded by one timeout.
async fn drive_request<T>(
    harness: &mut DeviceRuntimeHarness,
    request: impl Future<Output = T> + Send + 'static,
) -> T
where
    T: Send + 'static,
{
    let request = tokio::spawn(request);
    for _ in 0..200 {
        if request.is_finished() {
            break;
        }
        crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, rq, context, runtime)
        });
        if request.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("api request did not finish")
        .expect("api request panicked")
}

/// A bearer header derived from `token` by flipping its last digit.
fn wrong_token(token: &str) -> String {
    let mut bytes = token.as_bytes().to_vec();
    let last = bytes.last_mut().expect("token is not empty");
    *last = if *last == b'0' { b'1' } else { b'0' };
    format!(
        "Bearer {}",
        String::from_utf8(bytes).expect("a decimal token stays valid UTF-8")
    )
}

#[cfg(feature = "emulator")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_companion_binds_an_ephemeral_port() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::Select(EntryId::OpenCompanion),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    drive_listener_start(&mut harness).await;
    let port = harness.context.api.local_addr().unwrap().port();
    assert_ne!(port, 0);
    assert!(crate::view::common::locate_by_id(harness.view.as_ref(), ViewId::Companion).is_some());
    assert!(harness.context.api.is_on());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wifi_off_takes_no_lease_and_does_not_listen() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Off);
    let error = start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .expect_err("off must refuse");
    assert!(matches!(error, StartError::WifiOff));
    assert!(!holds_public_api_lease(&harness));
    assert!(!harness.context.api.is_on());
    assert!(harness.context.api.local_addr().is_none());
    assert_eq!(harness.context.settings.wifi, WifiMode::Off);
}

#[cfg(feature = "emulator")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_companion_with_wifi_off_notifies_instead_of_showing_a_code() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Off);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::Select(EntryId::OpenCompanion),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    let events = harness.drain_hub();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Notification(crate::view::NotificationEvent::Show(_))
    )));
    assert!(crate::view::common::locate_by_id(harness.view.as_ref(), ViewId::Companion).is_none());
    assert!(!harness.context.api.is_on());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auto_acquires_companion_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("auto starts");
    assert!(holds_public_api_lease(&harness));
    assert!(harness.context.api.is_on());
    assert!(harness.context.api.local_addr().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn always_on_acquires_companion_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("always on starts");
    assert!(holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_view_stops_the_listener() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::Close(ViewId::Companion),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert!(matches!(outcome, EventOutcome::Continue));
    assert!(!holds_public_api_lease(&harness));
    assert!(!harness.context.api.is_on());
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancels_the_job_and_drops_the_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    crate::api::shutdown(&mut harness.context).await;
    assert!(!holds_public_api_lease(&harness));
    assert!(!harness.context.api.is_on());
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_share_stops_the_listener_and_drops_the_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareShare, hub, bus, rq, context, runtime)
    });
    assert!(!harness.context.api.is_on());
    assert!(harness.context.api.local_addr().is_none());
    assert!(!holds_public_api_lease(&harness));
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usb_share_closes_the_view() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::PrepareShare, hub, bus, rq, context, runtime)
    });
    let events = harness.drain_hub();
    assert!(!harness.context.api.is_on());
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Close(ViewId::Companion)))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_resume_closes_the_view() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        suspend(context)
    });
    arm_wifi(&mut harness, WifiMode::Off);
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::ApiResume, hub, bus, rq, context, runtime)
    });
    let events = harness.drain_hub();
    assert!(!harness.context.api.is_on());
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Notification(crate::view::NotificationEvent::Show(_))
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Close(ViewId::Companion)))
    );
}

#[cfg(feature = "emulator")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_companion_replaces_a_running_session() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let old = harness.context.api.local_addr().unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::Select(EntryId::OpenCompanion),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    drive_listener_start(&mut harness).await;
    assert!(harness.context.api.is_on());
    assert!(harness.context.api.local_addr().is_some());
    assert!(tokio::net::TcpStream::connect(old).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_off_then_on_before_the_pending_start_drains_keeps_only_the_new_listener() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    crate::poll_parts!(harness, |hub, _bus, _rq, context, _runtime| {
        turn_on(hub, context, "127.0.0.1:0".parse().unwrap(), true)
    })
    .unwrap();
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        turn_off(context)
    });
    crate::poll_parts!(harness, |hub, _bus, _rq, context, _runtime| {
        turn_on(hub, context, "127.0.0.1:0".parse().unwrap(), true)
    })
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.api.is_on());
    assert!(harness.context.api.local_addr().is_some());
    assert!(holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspend_and_resume_keep_the_token() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let token = exposed_token(&harness);
    let addr = harness.context.api.local_addr().unwrap();
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        suspend(context)
    });
    assert_eq!(exposed_token(&harness), token);
    assert!(harness.context.api.is_on());
    assert!(!holds_public_api_lease(&harness));
    assert!(harness.context.api.local_addr().is_none());
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::ApiResume, hub, bus, rq, context, runtime)
    });
    drive_listener_start(&mut harness).await;
    assert_eq!(exposed_token(&harness), token);
    assert!(holds_public_api_lease(&harness));
    let again = harness.context.api.local_addr().unwrap();
    tokio::net::TcpStream::connect(again).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn companion_keeps_the_device_awake_until_it_stops() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    assert!(!harness.context.inhibitor.sleep_active());
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.inhibitor.sleep_active());
    assert!(super::public::companion_keeps_awake(&harness.context));

    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        suspend(context)
    });
    assert!(!harness.context.inhibitor.sleep_active());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::ApiResume, hub, bus, rq, context, runtime)
    });
    drive_listener_start(&mut harness).await;
    assert!(harness.context.inhibitor.sleep_active());

    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(
            &Event::Close(ViewId::Companion),
            hub,
            bus,
            rq,
            context,
            runtime,
        )
    });
    assert!(!harness.context.inhibitor.sleep_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn companion_starts_without_the_awake_lease_on_a_low_battery() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    harness
        .context
        .device
        .battery()
        .set_capacity(crate::device::inhibitor::INHIBIT_MIN_CAPACITY_PERCENT - 1.0);
    assert!(!harness.context.inhibitor.sleep_active());
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.api.local_addr().is_some());
    assert!(!harness.context.inhibitor.sleep_active());
    assert!(!super::public::companion_keeps_awake(&harness.context));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_and_a_blocked_start_release_the_awake_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.inhibitor.sleep_active());
    crate::api::shutdown(&mut harness.context).await;
    assert!(!harness.context.inhibitor.sleep_active());

    let mut busy = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut busy, WifiMode::Auto);
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap();
    start(&mut busy, addr).await.unwrap();
    assert!(busy.context.api.local_addr().is_none());
    assert!(!busy.context.inhibitor.sleep_active());
    assert!(!holds_public_api_lease(&busy));
    drop(taken);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_update_applies_on_the_main_loop_and_leaves_wifi_alone() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.settings.sleep_cover);
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::List { reply: tx }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    let catalog = rx.await.unwrap().expect("list");
    assert!(catalog.categories.iter().all(|category| {
        category.id.as_known() != Some(proto::CategoryId::CATEGORY_ID_DICTIONARIES)
    }));
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SLEEP_COVER),
            value: toggle_value(false),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap().expect("update applied");
    assert!(!harness.context.settings.sleep_cover);

    let catalog = send_list(&mut harness).await;
    let wifi_labels = [
        fl!("settings-wifi-mode-off"),
        fl!("settings-wifi-mode-always-on"),
        fl!("settings-wifi-mode-auto"),
    ];
    assert!(
        catalog
            .categories
            .iter()
            .flat_map(|category| &category.settings)
            .filter_map(|setting| setting.value.clone().into_option())
            .filter_map(|value| match value.kind {
                Some(proto::setting_value::Kind::Choice(choice)) => Some(*choice),
                _ => None,
            })
            .flat_map(|choice| choice.options)
            .all(|option| !wifi_labels.contains(&option.label)),
        "the settings catalog must not offer the Wi-Fi mode choices"
    );
    assert_eq!(harness.context.settings.wifi, WifiMode::AlwaysOn);
    assert!(holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn intermission_screens_are_editable_and_fill_color_accepts_rgb() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let catalog = send_list(&mut harness).await;
    assert_eq!(
        listed_choice(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_FILL_COLOR,
        )
        .selected,
        "#ffffff"
    );

    let cover = crate::settings::IntermissionDisplay::Cover.to_i18n_string();
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND),
        settings::choice_value(&cover, &[]),
    )
    .await
    .expect("suspend screen");
    assert_eq!(
        harness.context.settings.intermissions[crate::settings::IntermKind::Suspend],
        crate::settings::IntermissionDisplay::Cover
    );

    let image_dir = tempfile::tempdir().unwrap();
    let image = image_dir.path().join("photo.png");
    std::fs::write(&image, b"png-bytes").unwrap();
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_SUSPEND),
        settings::path_value(image.to_str().unwrap()),
    )
    .await
    .expect("custom image");
    assert_eq!(
        harness.context.settings.intermissions[crate::settings::IntermKind::Suspend],
        crate::settings::IntermissionDisplay::Image(image)
    );

    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_INTERMISSION_FILL_COLOR),
        settings::choice_value("#804020", &[]),
    )
    .await
    .expect("fill color");
    assert_eq!(
        harness.context.settings.intermissions.fill_color(),
        crate::color::Color::Rgb(128, 64, 32)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_update_rejects_values_the_setting_cannot_store() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::List { reply: tx }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    let catalog = rx.await.unwrap().expect("list");
    assert_eq!(
        listed_text(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SETTINGS_RETENTION,
        )
        .mode,
        proto::TextMode::TEXT_MODE_INTEGER
    );
    assert_eq!(
        listed_text(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_WIFI_IDLE_TIMEOUT,
        )
        .mode,
        proto::TextMode::TEXT_MODE_NUMBER
    );
    assert_eq!(
        listed_text(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_NTP_SERVER,
        )
        .mode,
        proto::TextMode::TEXT_MODE_HOSTNAME
    );
    assert_eq!(
        listed_text(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT_MANUAL_COORDINATES,
        )
        .mode,
        proto::TextMode::TEXT_MODE_COORDINATES
    );

    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SETTINGS_RETENTION,
            "nope"
        )
        .await
        .is_err()
    );
    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SETTINGS_RETENTION,
            "7.5"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.settings_retention, 3);
    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SETTINGS_RETENTION,
        "7",
    )
    .await
    .expect("integer saved");
    assert_eq!(harness.context.settings.settings_retention, 7);

    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_WIFI_IDLE_TIMEOUT,
            "later"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.wifi_idle_timeout, 5.0);
    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_WIFI_IDLE_TIMEOUT,
        "15.5",
    )
    .await
    .expect("number saved");
    assert_eq!(harness.context.settings.wifi_idle_timeout, 15.5);

    let auto_suspend = harness.context.settings.auto_suspend;
    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_AUTO_SUSPEND,
            "1e30"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.auto_suspend, auto_suspend);

    let auto_power_off = harness.context.settings.auto_power_off;
    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_AUTO_POWER_OFF,
            "1e30"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.auto_power_off, auto_power_off);

    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_WIFI_IDLE_TIMEOUT,
            "1e30"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.wifi_idle_timeout, 15.5);

    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT_MANUAL_COORDINATES,
            "91, 0"
        )
        .await
        .is_err()
    );
    assert!(
        harness
            .context
            .settings
            .auto_frontlight_manual_coordinates
            .is_none()
    );
    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_AUTO_FRONTLIGHT_MANUAL_COORDINATES,
        "10, 20",
    )
    .await
    .expect("coordinates saved");
    assert_eq!(
        harness.context.settings.auto_frontlight_manual_coordinates,
        Some(crate::geolocation::Coordinates::new(10.0, 20.0).unwrap())
    );

    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_NTP_SERVER,
        "pool.ntp.org",
    )
    .await
    .expect("hostname saved");
    assert_eq!(harness.context.settings.ntp_server.as_str(), "pool.ntp.org");
    assert!(
        update_text(
            &mut harness,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_NTP_SERVER,
            "!!!"
        )
        .await
        .is_err()
    );
    assert_eq!(harness.context.settings.ntp_server.as_str(), "pool.ntp.org");
    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_NTP_SERVER,
        "192.168.1.1",
    )
    .await
    .expect("ipv4 saved");
    assert_eq!(harness.context.settings.ntp_server.as_str(), "192.168.1.1");
    update_text(
        &mut harness,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_NTP_SERVER,
        "",
    )
    .await
    .expect("empty restores default");
    assert_eq!(
        harness.context.settings.ntp_server.as_str(),
        "time.cloudflare.com"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locale_choice_selects_display_tag_not_debug_ids() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.settings.locale.is_none());

    let catalog = send_list(&mut harness).await;
    let locale = listed_choice(
        &catalog,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LOCALE,
    );
    assert_eq!(locale.selected, DEFAULT_LOCALE);
    assert!(!locale.selected.is_empty());
    assert!(
        locale
            .options
            .iter()
            .any(|option| option.id == DEFAULT_LOCALE && option.label == DEFAULT_LOCALE)
    );
    assert!(
        locale
            .options
            .iter()
            .all(|option| option.id == option.label && !option.id.contains("SetLocale"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn choice_option_ids_are_labels() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    let layouts = harness.context.device.install_path("keyboard-layouts");
    tokio::fs::create_dir_all(&layouts).await.unwrap();
    tokio::fs::write(layouts.join("english.json"), b"{}")
        .await
        .unwrap();
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let catalog = send_list(&mut harness).await;
    for kind in [
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_KEYBOARD_LAYOUT,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_STARTUP_MODE,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_FONT_FAMILY,
    ] {
        let choice = listed_choice(&catalog, kind);
        assert!(!choice.options.is_empty(), "{kind:?} should list options");
        assert!(
            choice
                .options
                .iter()
                .all(|option| option.id == option.label),
            "{kind:?} option ids must equal labels: {:?}",
            choice.options
        );
        assert!(
            choice
                .options
                .iter()
                .all(|option| !option.id.contains("SetKeyboardLayout")
                    && !option.id.contains("SetStartupMode")
                    && !option.id.contains("SetDefaultFontFamily")),
            "{kind:?} must not use EntryId Debug as option id"
        );
    }
    let keyboard = listed_choice(
        &catalog,
        proto::SettingIdentityKind::SETTING_IDENTITY_KIND_KEYBOARD_LAYOUT,
    );
    assert!(keyboard.options.iter().any(|option| option.id == "English"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dithered_and_allowed_kinds_list_all_extensions_and_update() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let catalog = send_list(&mut harness).await;
    assert_checks_cover_all(
        &listed_checks(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_DITHERED_KINDS,
        ),
        &harness.context.settings.reader.dithered_kinds,
    );
    assert_checks_cover_all(
        &listed_checks(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ALLOWED_KINDS,
        ),
        &harness.context.settings.import.allowed_kinds,
    );

    assert!(
        !harness
            .context
            .settings
            .reader
            .dithered_kinds
            .contains(&FileExtension::Pdf)
    );
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_DITHERED_KINDS),
        settings::checks_value(&[("pdf", "PDF", true)]),
    )
    .await
    .expect("toggle pdf on");
    assert!(
        harness
            .context
            .settings
            .reader
            .dithered_kinds
            .contains(&FileExtension::Pdf)
    );
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_DITHERED_KINDS),
        settings::checks_value(&[("pdf", "PDF", false)]),
    )
    .await
    .expect("toggle pdf off");
    assert!(
        !harness
            .context
            .settings
            .reader
            .dithered_kinds
            .contains(&FileExtension::Pdf)
    );

    let before = harness.context.settings.reader.dithered_kinds.clone();
    let error = send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_DITHERED_KINDS),
        settings::checks_value(&[("not-a-type", "NOPE", true)]),
    )
    .await
    .expect_err("unknown extension");
    assert_eq!(error.code, connectrpc::ErrorCode::InvalidArgument);
    assert_eq!(harness.context.settings.reader.dithered_kinds, before);

    assert!(
        harness
            .context
            .settings
            .import
            .allowed_kinds
            .contains(&FileExtension::Pdf)
    );
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ALLOWED_KINDS),
        settings::checks_value(&[("pdf", "PDF", false)]),
    )
    .await
    .expect("toggle allowed pdf off");
    assert!(
        !harness
            .context
            .settings
            .import
            .allowed_kinds
            .contains(&FileExtension::Pdf)
    );
    let allowed_before = harness.context.settings.import.allowed_kinds.clone();
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ALLOWED_KINDS),
        settings::checks_value(&[("zzz", "ZZZ", true)]),
    )
    .await
    .expect_err("unknown allowed kind");
    assert_eq!(
        harness.context.settings.import.allowed_kinds,
        allowed_before
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_kinds_list_extension_ids_and_update() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let catalog = send_list(&mut harness).await;
    assert_checks_cover_all(
        &listed_checks(
            &catalog,
            proto::SettingIdentityKind::SETTING_IDENTITY_KIND_METADATA_KINDS,
        ),
        &harness.context.settings.import.metadata_kinds,
    );

    let before = harness
        .context
        .settings
        .import
        .metadata_kinds
        .contains(&FileExtension::Pdf);
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_METADATA_KINDS),
        settings::checks_value(&[("pdf", "PDF", !before)]),
    )
    .await
    .expect("toggle pdf");
    assert_eq!(
        harness
            .context
            .settings
            .import
            .metadata_kinds
            .contains(&FileExtension::Pdf),
        !before
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_full_import_is_omitted_from_the_catalog() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let catalog = send_list(&mut harness).await;
    let import = catalog
        .categories
        .iter()
        .find(|category| category.id.as_known() == Some(proto::CategoryId::CATEGORY_ID_IMPORT))
        .expect("import category");
    assert!(import.settings.iter().all(|setting| {
        setting.id.kind.as_known()
            != Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_FORCE_FULL_IMPORT)
    }));
    assert!(import.settings.iter().any(|setting| {
        setting.id.kind.as_known()
            == Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ALLOWED_KINDS)
    }));
    assert!(import.settings.iter().any(|setting| {
        setting.id.kind.as_known()
            == Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_IMPORT_SYNC_METADATA)
    }));

    let error = send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_FORCE_FULL_IMPORT),
        toggle_value(true),
    )
    .await
    .expect_err("force import is not in the catalog");
    assert_eq!(error.code, connectrpc::ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn libraries_list_as_library_value_and_add_remove_paths() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let (first_name, first_path, library_count) = {
        let first = &harness.context.settings.libraries[0];
        (
            first.name.clone(),
            std::path::absolute(&first.path).expect("absolute library path"),
            harness.context.settings.libraries.len(),
        )
    };

    let catalog = send_list(&mut harness).await;
    let listed = listed_setting(&catalog, lid(0));
    let value = listed.value.clone().into_option().unwrap_or_default();
    let proto::setting_value::Kind::Library(library) = value.kind.as_ref().expect("library kind")
    else {
        panic!("LibraryInfo listed as {:?}", value.kind);
    };
    assert_eq!(library.name, first_name);
    let listed_path = std::path::Path::new(library.path.path.as_str());
    assert!(listed_path.is_absolute(), "{}", library.path.path);
    assert_eq!(listed_path, first_path.as_path());

    let added_dir = tempfile::tempdir().unwrap();
    let added_path = added_dir.path().to_string_lossy().into_owned();
    send_add_library(&mut harness, library_payload("Books", &added_path, ""))
        .await
        .expect("add library");
    assert_eq!(harness.context.settings.libraries.len(), library_count + 1);
    assert_eq!(
        harness.context.settings.libraries[library_count].name,
        "Books"
    );
    assert_eq!(
        harness.context.settings.libraries[library_count].path,
        added_dir.path()
    );

    let updated_dir = tempfile::tempdir().unwrap();
    let updated_path = updated_dir.path().to_string_lossy().into_owned();
    send_update(
        &mut harness,
        lid(0),
        library_update(&first_name, &updated_path, ""),
    )
    .await
    .expect("path update");
    assert_eq!(
        harness.context.settings.libraries[0].path,
        updated_dir.path()
    );

    let file = tempfile::NamedTempFile::new().unwrap();
    send_update(
        &mut harness,
        lid(0),
        library_update(&first_name, file.path().to_str().unwrap(), ""),
    )
    .await
    .expect_err("file path rejected");
    assert_eq!(
        harness.context.settings.libraries[0].path,
        updated_dir.path()
    );

    send_remove_library(&mut harness, lid(library_count as u32))
        .await
        .expect("remove library");
    assert_eq!(harness.context.settings.libraries.len(), library_count);
    assert_eq!(harness.context.settings.libraries[0].name, first_name);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_library_paths_reject_stale_row_ids() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let books = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let books_path = books.path().to_str().unwrap().to_string();
    let library = |name: &str, path: &std::path::Path| crate::settings::LibrarySettings {
        name: name.to_string(),
        path: path.to_path_buf(),
        ..Default::default()
    };
    harness.context.settings.libraries = vec![
        library("Alpha", books.path()),
        library("Beta", other.path()),
        library("Gamma", books.path()),
    ];
    harness.context.settings.selected_library = 0;

    let catalog = send_list(&mut harness).await;
    let beta = listed_setting(&catalog, lid(1))
        .id
        .clone()
        .into_option()
        .unwrap();
    let gamma = listed_setting(&catalog, lid(2))
        .id
        .clone()
        .into_option()
        .unwrap();
    send_remove_library(&mut harness, beta)
        .await
        .expect("remove beta");

    send_update(
        &mut harness,
        proto::SettingId {
            index: Some(2),
            ..gamma.clone()
        },
        library_update("Stale", &books_path, ""),
    )
    .await
    .expect_err("stale row id is rejected");
    assert_eq!(harness.context.settings.libraries[0].name, "Alpha");
    assert_eq!(harness.context.settings.libraries[1].name, "Gamma");

    send_update(
        &mut harness,
        proto::SettingId {
            index: Some(1),
            ..gamma
        },
        library_update("Gamma renamed", &books_path, ""),
    )
    .await
    .expect("current row id updates");
    assert_eq!(harness.context.settings.libraries[0].name, "Alpha");
    assert_eq!(harness.context.settings.libraries[1].name, "Gamma renamed");
}

fn listed_text(
    catalog: &proto::SettingsCatalog,
    id: proto::SettingIdentityKind,
) -> proto::TextValue {
    let setting = listed_setting(catalog, sid(id));
    let value = setting.value.clone().into_option().unwrap_or_default();
    match value.kind {
        Some(proto::setting_value::Kind::Text(text)) => *text,
        other => panic!("{id:?} listed as {other:?}"),
    }
}

async fn update_text(
    harness: &mut DeviceRuntimeHarness,
    id: proto::SettingIdentityKind,
    text: &str,
) -> Result<proto::Setting, ConnectError> {
    send_update(
        harness,
        sid(id),
        proto::SettingValue {
            kind: Some(proto::setting_value::Kind::Text(Box::new(
                proto::TextValue {
                    text: text.to_string(),
                    ..Default::default()
                },
            ))),
            ..Default::default()
        },
    )
    .await
}

async fn send_add_library(
    harness: &mut DeviceRuntimeHarness,
    library: proto::LibraryValue,
) -> Result<proto::Setting, ConnectError> {
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::AddLibrary {
            library,
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap()
}

async fn send_remove_library(
    harness: &mut DeviceRuntimeHarness,
    id: proto::SettingId,
) -> Result<(), ConnectError> {
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::RemoveLibrary {
            id,
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap()
}

async fn send_list(harness: &mut DeviceRuntimeHarness) -> proto::SettingsCatalog {
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::List { reply: tx }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap().expect("list")
}

fn sid(kind: proto::SettingIdentityKind) -> proto::SettingId {
    proto::SettingId {
        kind: kind.into(),
        ..Default::default()
    }
}

fn lid(index: u32) -> proto::SettingId {
    proto::SettingId {
        kind: proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LIBRARY_INFO.into(),
        index: Some(index),
        ..Default::default()
    }
}

async fn send_update(
    harness: &mut DeviceRuntimeHarness,
    id: proto::SettingId,
    value: proto::SettingValue,
) -> Result<proto::Setting, ConnectError> {
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id,
            value,
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap()
}

fn listed_setting(catalog: &proto::SettingsCatalog, id: proto::SettingId) -> &proto::Setting {
    catalog
        .categories
        .iter()
        .flat_map(|category| &category.settings)
        .find(|setting| {
            setting.id.kind == id.kind
                && setting.id.index == id.index
                && (id.key.is_none() || setting.id.key == id.key)
        })
        .unwrap_or_else(|| panic!("missing {id:?}"))
}

fn listed_choice(
    catalog: &proto::SettingsCatalog,
    kind: proto::SettingIdentityKind,
) -> proto::ChoiceValue {
    let value = listed_setting(catalog, sid(kind))
        .value
        .clone()
        .into_option()
        .unwrap_or_default();
    match value.kind {
        Some(proto::setting_value::Kind::Choice(choice)) => *choice,
        other => panic!("{kind:?} listed as {other:?}"),
    }
}

fn listed_checks(
    catalog: &proto::SettingsCatalog,
    kind: proto::SettingIdentityKind,
) -> proto::ChecksValue {
    let value = listed_setting(catalog, sid(kind))
        .value
        .clone()
        .into_option()
        .unwrap_or_default();
    match value.kind {
        Some(proto::setting_value::Kind::Checks(checks)) => *checks,
        other => panic!("{kind:?} listed as {other:?}"),
    }
}

fn assert_checks_cover_all(checks: &proto::ChecksValue, on: &rustc_hash::FxHashSet<FileExtension>) {
    assert_eq!(checks.items.len(), FileExtension::all().len());
    for kind in FileExtension::all() {
        let item = checks
            .items
            .iter()
            .find(|item| item.id == kind.as_str())
            .unwrap_or_else(|| panic!("missing {}", kind.as_str()));
        assert_eq!(item.on, on.contains(kind));
        assert_eq!(item.label, kind.as_str().to_uppercase());
    }
}

fn library_update(name: &str, path: &str, finished: &str) -> proto::SettingValue {
    settings::library_value(
        name,
        path,
        proto::ChoiceValue {
            selected: finished.to_string(),
            ..Default::default()
        },
    )
}

fn library_payload(name: &str, path: &str, finished: &str) -> proto::LibraryValue {
    match library_update(name, path, finished).kind {
        Some(proto::setting_value::Kind::Library(library)) => *library,
        _ => panic!("library value expected"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_rate_update_sets_global_and_per_file_type() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::List { reply: tx }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    let catalog = rx.await.unwrap().expect("list");
    let listed = catalog
        .categories
        .iter()
        .flat_map(|category| &category.settings)
        .find(|setting| {
            setting.id.kind.as_known()
                == Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_REFRESH_RATE)
        })
        .expect("refresh rate row");
    let listed = listed.value.clone().into_option().unwrap_or_default();
    match listed.kind {
        Some(proto::setting_value::Kind::RefreshRate(rate)) => {
            assert_eq!(rate.regular, 8);
            assert_eq!(rate.inverted, 2);
            assert!(rate.by_kind.is_empty());
            assert!(
                rate.available
                    .iter()
                    .any(|extension| extension.as_known()
                        == Some(proto::Extension::EXTENSION_EPUB))
            );
        }
        other => panic!("refresh rate listed as {other:?}"),
    }

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_REFRESH_RATE),
            value: refresh_rate_update(4, 1, &[("epub", 2, 3)]),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    let updated = rx.await.unwrap().expect("update applied");
    assert_eq!(
        harness.context.settings.reader.refresh_rate.global.regular,
        4
    );
    assert_eq!(
        harness.context.settings.reader.refresh_rate.global.inverted,
        1
    );
    let epub = harness
        .context
        .settings
        .reader
        .refresh_rate
        .by_kind
        .get(&crate::document::file_extension::FileExtension::Epub)
        .expect("epub pair");
    assert_eq!(epub.regular, 2);
    assert_eq!(epub.inverted, 3);
    let updated = updated.value.into_option().unwrap_or_default();
    match updated.kind {
        Some(proto::setting_value::Kind::RefreshRate(rate)) => {
            assert_eq!(rate.by_kind.len(), 1);
            assert!(
                !rate
                    .available
                    .iter()
                    .any(|extension| extension.as_known()
                        == Some(proto::Extension::EXTENSION_EPUB))
            );
        }
        other => panic!("refresh rate updated as {other:?}"),
    }

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_REFRESH_RATE),
            value: refresh_rate_update(4, 1, &[("not-a-type", 1, 1)]),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(rx.await.unwrap().is_err());
    assert!(
        harness
            .context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .contains_key(&crate::document::file_extension::FileExtension::Epub)
    );

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_REFRESH_RATE),
            value: refresh_rate_update(256, 1, &[]),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(rx.await.unwrap().is_err());
    assert_eq!(
        harness.context.settings.reader.refresh_rate.global.regular,
        4
    );

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_REFRESH_RATE),
            value: refresh_rate_update(4, 1, &[]),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    rx.await.unwrap().expect("cleared file types");
    assert!(
        harness
            .context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .is_empty()
    );
}

fn refresh_rate_update(
    regular: u32,
    inverted: u32,
    by_kind: &[(&str, u32, u32)],
) -> proto::SettingValue {
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::RefreshRate(Box::new(
            proto::RefreshRateValue {
                regular,
                inverted,
                by_kind: by_kind
                    .iter()
                    .map(|(extension, regular, inverted)| proto::RefreshRateByKind {
                        extension: match (*extension).parse::<FileExtension>() {
                            Ok(known) => proto::Extension::from(known).into(),
                            Err(_) => 999.into(),
                        },
                        regular: *regular,
                        inverted: *inverted,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_rpc_rejects_a_missing_bearer_token() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let token = exposed_token(&harness).expect("token");
    let wrong = wrong_token(&token);
    let missing = drive_request(&mut harness, async move { http_status(addr, None).await }).await;
    assert!(
        missing.starts_with("HTTP/1.1 401"),
        "missing token status: {missing}"
    );
    let wrong = drive_request(&mut harness, async move {
        http_status(addr, Some(&wrong)).await
    })
    .await;
    assert!(
        wrong.starts_with("HTTP/1.1 401"),
        "wrong token status: {wrong}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_bad_bearer_credentials_shut_the_public_listener_down() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let token = exposed_token(&harness).expect("token");
    for _ in 0..MAX_AUTH_FAILURES {
        let wrong = wrong_token(&token);
        let status = http_status(addr, Some(&wrong)).await;
        assert!(
            status.starts_with("HTTP/1.1 401"),
            "wrong token status: {status}"
        );
    }
    let valid = format!("Bearer {token}");
    let locked = http_status(addr, Some(&valid)).await;
    assert!(
        locked.starts_with("HTTP/1.1 503"),
        "a locked listener rejects even the valid token before the main loop drains: {locked}"
    );
    assert!(harness.context.api.is_on());
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(
        harness
            .drain_hub()
            .iter()
            .any(|event| matches!(event, Event::Close(ViewId::Companion))),
        "the lockout closes the companion view"
    );
    assert!(!harness.context.api.is_on());
    assert!(!holds_public_api_lease(&harness));
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_request_clears_bad_token_strikes() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let token = exposed_token(&harness).expect("token");
    let addr = harness.context.api.local_addr().unwrap();
    for _ in 0..(MAX_AUTH_FAILURES - 1) {
        let wrong = wrong_token(&token);
        let status = http_status(addr, Some(&wrong)).await;
        assert!(
            status.starts_with("HTTP/1.1 401"),
            "wrong token status: {status}"
        );
    }
    let valid = format!("Bearer {token}");
    let ok = drive_request(&mut harness, async move {
        http_status(addr, Some(&valid)).await
    })
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "valid token status: {ok}");
    for _ in 0..(MAX_AUTH_FAILURES - 1) {
        let wrong = wrong_token(&token);
        let status = http_status(addr, Some(&wrong)).await;
        assert!(
            status.starts_with("HTTP/1.1 401"),
            "wrong token status: {status}"
        );
    }
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.api.is_on());
    assert!(holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headerless_requests_do_not_trip_the_lockout() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    for _ in 0..(MAX_AUTH_FAILURES + 3) {
        let missing =
            drive_request(&mut harness, async move { http_status(addr, None).await }).await;
        assert!(
            missing.starts_with("HTTP/1.1 401"),
            "missing token status: {missing}"
        );
    }
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(harness.context.api.is_on());
    assert!(holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_token_strikes_survive_suspend_and_resume() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let token = exposed_token(&harness).expect("token");
    let addr = harness.context.api.local_addr().unwrap();
    for _ in 0..(MAX_AUTH_FAILURES - 1) {
        let wrong = wrong_token(&token);
        let status = drive_request(&mut harness, async move {
            http_status(addr, Some(&wrong)).await
        })
        .await;
        assert!(
            status.starts_with("HTTP/1.1 401"),
            "wrong token status: {status}"
        );
    }
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        suspend(context)
    });
    assert!(!holds_public_api_lease(&harness));
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::ApiResume, hub, bus, rq, context, runtime)
    });
    drive_listener_start(&mut harness).await;
    assert_eq!(exposed_token(&harness), Some(token.clone()));
    let addr = harness.context.api.local_addr().unwrap();
    let wrong = wrong_token(&token);
    let status = http_status(addr, Some(&wrong)).await;
    assert!(
        status.starts_with("HTTP/1.1 401"),
        "wrong token status: {status}"
    );
    crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, rq, context, runtime)
    });
    assert!(
        !harness.context.api.is_on(),
        "the third strike survives resume"
    );
    assert!(!holds_public_api_lease(&harness));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_writes_fail_while_storage_is_shared() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(harness.context.settings.sleep_cover);
    harness.context.shared = true;
    let catalog = send_list(&mut harness).await;
    assert!(!catalog.categories.is_empty(), "listing still works");
    let error = send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SLEEP_COVER),
        toggle_value(false),
    )
    .await
    .expect_err("shared storage refuses writes");
    assert_eq!(error.code, connectrpc::ErrorCode::Unavailable);
    assert!(harness.context.settings.sleep_cover);
    harness.context.shared = false;
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SLEEP_COVER),
        toggle_value(false),
    )
    .await
    .expect("writes resume after the share ends");
    assert!(!harness.context.settings.sleep_cover);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_reports_the_compile_time_version() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let expected = crate::version::get_current_version();
    let body = drive_request(&mut harness, async move {
        http_exchange(
            addr,
            "POST /cadmus.api.shared.v1.Status/GetStatus HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        )
        .await
    })
    .await;
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(body.contains("LOCALE_EN"), "{body}");
    assert!(
        body.contains(&format!("\"minor\":\"{}\"", expected.minor()))
            || body.contains(&format!("\"minor\":{}", expected.minor())),
        "{body}"
    );
    if let Some(hash) = expected.hash() {
        assert!(body.contains(&format!("\"commit\":\"{hash}\"")), "{body}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn site_files_need_no_token_and_a_valid_bearer_lists_settings() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    let site = harness.context.device.install_path("website");
    let shared = site.join("_shared/api");
    tokio::fs::create_dir_all(&shared).await.unwrap();
    tokio::fs::write(site.join("hello.txt"), b"guide")
        .await
        .unwrap();
    tokio::fs::write(shared.join("index.html"), b"api-docs")
        .await
        .unwrap();
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let page = http_get(addr, "/hello.txt").await;
    let api = http_get(addr, "/en/api/index.html").await;
    tokio::fs::create_dir_all(site.join("storybook"))
        .await
        .unwrap();
    tokio::fs::write(site.join("storybook/index.html"), b"story")
        .await
        .unwrap();
    let story = http_get(addr, "/storybook/index.html").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("guide"), "{page}");
    assert!(api.starts_with("HTTP/1.1 200"), "{api}");
    assert!(api.contains("api-docs"), "{api}");
    assert!(story.starts_with("HTTP/1.1 404"), "{story}");

    let token = harness
        .context
        .api
        .token
        .as_ref()
        .expect("token")
        .expose_secret()
        .to_string();
    let listed = drive_request(&mut harness, async move {
        http_status(addr, Some(&format!("Bearer {token}"))).await
    })
    .await;
    assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bare_locale_path_redirects_to_the_companion_page() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    let site = harness.context.device.install_path("website");
    tokio::fs::create_dir_all(site.join("xx/companion"))
        .await
        .unwrap();
    tokio::fs::write(site.join("xx/companion/index.html"), b"companion")
        .await
        .unwrap();
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();

    let home = http_get(addr, "/xx/").await.to_ascii_lowercase();
    assert!(home.starts_with("http/1.1 302"), "{home}");
    assert!(home.contains("location: /xx/companion/"), "{home}");

    let page = http_get(addr, "/xx/companion/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("companion"), "{page}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gzipped_site_files_are_sent_encoded_or_inflated_by_accept_encoding() {
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    let shared = harness
        .context
        .device
        .install_path("website")
        .join("_shared/api");
    tokio::fs::create_dir_all(&shared).await.unwrap();
    let page = "<p>api docs</p>".repeat(100);
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(page.as_bytes()).unwrap();
    let compressed = encoder.finish().unwrap();
    tokio::fs::write(shared.join("gzip-only.html.gz"), &compressed)
        .await
        .unwrap();
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();

    async fn raw_get(addr: SocketAddr, accept_encoding: &str, method: &str) -> Vec<u8> {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(
                format!(
                    "{method} /en/api/gzip-only.html HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: {accept_encoding}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
            .await
            .expect("response read timed out")
            .unwrap();
        buf
    }

    fn split(response: &[u8]) -> (String, &[u8]) {
        let end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("headers end");
        (
            String::from_utf8_lossy(&response[..end]).to_ascii_lowercase(),
            &response[end + 4..],
        )
    }

    let encoded = raw_get(addr, "gzip, br", "GET").await;
    let (headers, body) = split(&encoded);
    assert!(headers.starts_with("http/1.1 200"), "{headers}");
    assert!(headers.contains("content-encoding: gzip"), "{headers}");
    assert!(headers.contains("vary: accept-encoding"), "{headers}");
    assert!(headers.contains("content-type: text/html"), "{headers}");
    assert!(
        headers.contains(&format!("content-length: {}", compressed.len())),
        "{headers}"
    );
    assert_eq!(body, compressed.as_slice());

    let plain = raw_get(addr, "identity", "GET").await;
    let (headers, body) = split(&plain);
    assert!(headers.starts_with("http/1.1 200"), "{headers}");
    assert!(!headers.contains("content-encoding"), "{headers}");
    assert!(headers.contains("vary: accept-encoding"), "{headers}");
    assert!(
        headers.contains(&format!("content-length: {}", page.len())),
        "{headers}"
    );
    assert_eq!(body, page.as_bytes());

    let head = raw_get(addr, "gzip", "HEAD").await;
    let (headers, body) = split(&head);
    assert!(headers.contains("content-encoding: gzip"), "{headers}");
    assert!(
        headers.contains(&format!("content-length: {}", compressed.len())),
        "{headers}"
    );
    assert!(body.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openapi_json_is_served_without_a_token() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let response = http_get(addr, "/openapi.json").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{response}"
    );
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("headers end");
    let document: serde_json::Value = serde_json::from_str(body).expect("openapi json");
    assert!(
        document["paths"]
            .get("/cadmus.api.shared.v1.Settings/List")
            .is_some(),
        "{body}"
    );
    assert!(
        document["paths"]
            .get("/cadmus.api.internal.v1.Placeholder/Ping")
            .is_some(),
        "private RPC missing from the document: {body}"
    );
    assert!(
        document["paths"]
            .get("/cadmus.api.public.v1.Logs/List")
            .is_some(),
        "Companion-only RPC missing from the document: {body}"
    );
    assert!(
        body.contains("Companion-only"),
        "Logs service description does not say Companion-only: {body}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_over_http2_serves_get_status() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let (status, content_type, body_len) =
        drive_request(&mut harness, async move { grpc_status(addr).await }).await;
    assert_eq!(status, 200);
    assert_eq!(content_type, "application/grpc+proto");
    assert!(body_len >= 5, "grpc frame header missing");
}

/// One gRPC call over cleartext HTTP/2 with prior knowledge.
async fn grpc_status(addr: SocketAddr) -> (u16, String, usize) {
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (client, connection) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut client = client.ready().await.unwrap();
    let request = http::Request::builder()
        .method(http::Method::POST)
        .uri(format!(
            "http://{addr}/cadmus.api.shared.v1.Status/GetStatus"
        ))
        .header(http::header::CONTENT_TYPE, "application/grpc+proto")
        .header("te", "trailers")
        .body(())
        .unwrap();
    let (response, mut send) = client.send_request(request, false).unwrap();
    send.send_data(bytes::Bytes::from_static(&[0, 0, 0, 0, 0]), true)
        .unwrap();
    let response = response.await.unwrap();
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut body = response.into_body();
    let mut body_len = 0;
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        body_len += chunk.len();
        let _ = body.flow_control().release_capacity(chunk.len());
    }
    (status, content_type, body_len)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_rpc_is_only_routed_on_the_loopback_listener() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, _bus, _rq, context, _runtime| {
        super::start_private(hub, context)
    });
    let public = harness.context.api.local_addr().unwrap();
    let private = harness.context.api.private_addr.unwrap();
    let token = exposed_token(&harness).expect("token");
    let authorization = format!("Bearer {token}");
    let (on_public, on_private) = tokio::join!(
        private_ping(public, Some(&authorization)),
        private_ping(private, None)
    );
    assert!(on_public.starts_with("HTTP/1.1 404"), "{on_public}");
    assert!(on_private.starts_with("HTTP/1.1 200"), "{on_private}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_are_only_routed_on_the_companion_listener() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    crate::poll_parts!(harness, |hub, _bus, _rq, context, _runtime| {
        super::start_private(hub, context)
    });
    let public = harness.context.api.local_addr().unwrap();
    let private = harness.context.api.private_addr.unwrap();
    let token = exposed_token(&harness).expect("token");
    let authorization = format!("Bearer {token}");
    let (with_pin, without_pin, on_loopback) = tokio::join!(
        logs_list(public, Some(&authorization)),
        logs_list(public, None),
        logs_list(private, None)
    );
    assert!(with_pin.starts_with("HTTP/1.1 200"), "{with_pin}");
    assert!(without_pin.starts_with("HTTP/1.1 401"), "{without_pin}");
    assert!(on_loopback.starts_with("HTTP/1.1 404"), "{on_loopback}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summarize_streams_to_done_and_reuses_the_cached_scan() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    let log_id = write_run_log(&harness);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let token = exposed_token(&harness).expect("token");
    let authorization = format!("Bearer {token}");

    let (head, body) = logs_summarize(addr, &authorization, log_id).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let first = summary_messages(&body);
    assert!(
        first.len() > 1,
        "the scan should stream snapshots: {}",
        first.len()
    );
    assert!(first.last().expect("a summary").done);
    let lines = first.last().expect("a summary").lines;

    let (head, body) = logs_summarize(addr, &authorization, log_id).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let second = summary_messages(&body);
    assert_eq!(second.len(), 1, "an unchanged log comes from the cache");
    assert!(second[0].done);
    assert_eq!(second[0].lines, lines);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summarize_of_an_unknown_log_is_not_found() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let token = exposed_token(&harness).expect("token");
    let authorization = format!("Bearer {token}");
    let (head, body) = logs_summarize(addr, &authorization, "no-such-log").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("not_found"), "{text}");
}

/// Writes a Run log large enough to stream more than one snapshot, and returns
/// the canonical run ID that names it.
fn write_run_log(harness: &DeviceRuntimeHarness) -> &'static str {
    const ID: &str = "01a20edc-8083-75d7-b900-35f14f330c68";
    const LINE: &str = "{\"timestamp\":\"2026-11-25T10:00:00Z\",\"level\":\"INFO\",\"fields\":{\"message\":\"line\"},\"target\":\"cadmus_core::device\"}\n";
    let dir = harness
        .context
        .device
        .data_path(&harness.context.settings.logging.directory);
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("cadmus-{ID}.json"))).unwrap();
    let mut writer = std::io::BufWriter::new(file);
    let mut written = 0usize;
    while written <= 9 * 1024 * 1024 {
        std::io::Write::write_all(&mut writer, LINE.as_bytes()).unwrap();
        written += LINE.len();
    }
    std::io::Write::flush(&mut writer).unwrap();
    ID
}

/// One `cadmus.api.public.v1.Logs/Summarize` call: response head and body.
async fn logs_summarize(addr: SocketAddr, authorization: &str, log_id: &str) -> (String, Vec<u8>) {
    let payload = format!("{{\"logId\":\"{log_id}\"}}");
    let mut request = format!(
        "POST /cadmus.api.public.v1.Logs/Summarize HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/connect+json\r\nConnect-Protocol-Version: 1\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        5 + payload.len()
    )
    .into_bytes();
    request.push(0);
    request.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    request.extend_from_slice(payload.as_bytes());
    http_parts(&http_exchange_bytes(addr, &request).await)
}

/// Splits raw response bytes into the header block and the decoded body.
fn http_parts(response: &[u8]) -> (String, Vec<u8>) {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response headers");
    let head = String::from_utf8_lossy(&response[..split]).to_string();
    let body = &response[split + 4..];
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    (
        head,
        if chunked {
            dechunk(body)
        } else {
            body.to_vec()
        },
    )
}

/// Bytes of a chunked HTTP body.
fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size line");
        let line = std::str::from_utf8(&body[..line_end]).expect("chunk size");
        let size = usize::from_str_radix(line.split(';').next().unwrap().trim(), 16)
            .expect("chunk size in hex");
        body = &body[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

/// Summaries carried by a Connect server-stream body.
fn summary_messages(body: &[u8]) -> Vec<public_proto::LogSummary> {
    let mut messages = Vec::new();
    let mut at = 0;
    while at + 5 <= body.len() {
        let flags = body[at];
        let len =
            u32::from_be_bytes([body[at + 1], body[at + 2], body[at + 3], body[at + 4]]) as usize;
        let payload = &body[at + 5..at + 5 + len];
        at += 5 + len;
        if flags & 0x02 != 0 {
            break;
        }
        messages.push(
            serde_json::from_slice::<public_proto::LogSummary>(payload)
                .expect("summary JSON message"),
        );
    }
    messages
}

/// Raw response bytes, headers and body.
async fn http_exchange_bytes(addr: SocketAddr, req: &[u8]) -> Vec<u8> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut buf))
        .await
        .expect("response read timed out")
        .unwrap();
    buf
}

/// `cadmus.api.public.v1.Logs/List`.
async fn logs_list(addr: SocketAddr, authorization: Option<&str>) -> String {
    let auth = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    http_exchange(
        addr,
        &format!(
            "POST /cadmus.api.public.v1.Logs/List HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\n{auth}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    )
    .await
}

/// `cadmus.api.internal.v1.Placeholder/Ping`, a no-op internal RPC.
async fn private_ping(addr: SocketAddr, authorization: Option<&str>) -> String {
    let auth = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    http_exchange(
        addr,
        &format!(
            "POST /cadmus.api.internal.v1.Placeholder/Ping HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\n{auth}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    )
    .await
}

#[test]
fn access_log_names_the_router() {
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Buffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buffer = Buffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .without_time()
        .with_ansi(false)
        .with_writer({
            let buffer = buffer.clone();
            move || buffer.clone()
        })
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        super::serve::log_access(
            super::serve::Scope::Internal,
            &http::Method::POST,
            "/cadmus.api.internal.v1.Placeholder/Ping",
            http::StatusCode::OK,
            Duration::from_millis(2),
        );
    });

    let logged = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("router=\"internal\""), "{logged}");
    assert!(logged.contains("status=200"), "{logged}");
    assert!(logged.contains("api request"), "{logged}");
}

async fn http_get(addr: SocketAddr, path: &str) -> String {
    http_exchange(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn http_status(addr: SocketAddr, authorization: Option<&str>) -> String {
    let auth = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    http_exchange(
        addr,
        &format!(
            "POST /cadmus.api.shared.v1.Settings/List HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\n{auth}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        ),
    )
    .await
}

async fn http_exchange(addr: SocketAddr, req: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
        .await
        .expect("response read timed out")
        .unwrap();
    String::from_utf8_lossy(&buf).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_removal_keeps_the_selection_valid() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    while harness.context.settings.libraries.len() > 1 {
        let last = harness.context.settings.libraries.len() - 1;
        send_remove_library(&mut harness, lid(last as u32))
            .await
            .expect("trim extra library");
    }

    let error = send_remove_library(&mut harness, lid(0))
        .await
        .expect_err("removing the only library is rejected");
    assert_eq!(error.code, connectrpc::ErrorCode::FailedPrecondition);
    assert_eq!(harness.context.settings.libraries.len(), 1);

    let extra_a = tempfile::tempdir().unwrap();
    send_add_library(
        &mut harness,
        library_payload("Alpha", extra_a.path().to_str().unwrap(), ""),
    )
    .await
    .expect("add alpha");
    let extra_b = tempfile::tempdir().unwrap();
    send_add_library(
        &mut harness,
        library_payload("Beta", extra_b.path().to_str().unwrap(), ""),
    )
    .await
    .expect("add beta");

    harness.context.settings.selected_library = 2;
    send_remove_library(&mut harness, lid(0))
        .await
        .expect("remove library below the selection");
    assert_eq!(harness.context.settings.selected_library, 1);

    let library_home = harness.context.library.home.clone();
    let error = send_remove_library(&mut harness, lid(1))
        .await
        .expect_err("the selected library cannot be removed");
    assert_eq!(error.code, connectrpc::ErrorCode::FailedPrecondition);
    assert_eq!(harness.context.settings.selected_library, 1);
    assert_eq!(harness.context.settings.libraries.len(), 2);
    assert_eq!(harness.context.library.home, library_home);

    harness.context.settings.selected_library = 0;
    send_remove_library(&mut harness, lid(1))
        .await
        .expect("remove a library below the selection");
    assert_eq!(harness.context.settings.selected_library, 0);
    assert_eq!(harness.context.settings.libraries.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_updates_resolve_by_path_key() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let alpha_dir = tempfile::tempdir().unwrap();
    send_add_library(
        &mut harness,
        library_payload("Alpha", alpha_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect("add alpha");

    let catalog = send_list(&mut harness).await;
    let alpha_key = catalog
        .categories
        .iter()
        .flat_map(|category| &category.settings)
        .find(|setting| {
            setting.id.kind.as_known()
                == Some(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LIBRARY_INFO)
                && setting.label == "Alpha"
        })
        .and_then(|setting| setting.id.key.clone())
        .expect("library row keyed by path");
    assert_eq!(
        std::path::Path::new(&alpha_key),
        std::path::absolute(alpha_dir.path()).unwrap().as_path()
    );

    harness.context.settings.selected_library = 1;
    send_remove_library(&mut harness, lid(0))
        .await
        .expect("remove the library before alpha");
    assert_eq!(harness.context.settings.selected_library, 0);

    let alpha_id = proto::SettingId {
        kind: proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LIBRARY_INFO.into(),
        key: Some(alpha_key),
        ..Default::default()
    };
    let renamed_dir = tempfile::tempdir().unwrap();
    send_update(
        &mut harness,
        alpha_id,
        library_update("Renamed", renamed_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect("update by key after index shift");
    let alpha = harness
        .context
        .settings
        .libraries
        .iter()
        .find(|library| library.name == "Renamed")
        .expect("alpha renamed by key");
    assert_eq!(alpha.path, renamed_dir.path());

    let stale_id = proto::SettingId {
        kind: proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LIBRARY_INFO.into(),
        key: Some("/gone".to_string()),
        ..Default::default()
    };
    let error = send_update(
        &mut harness,
        stale_id,
        library_update("Renamed", renamed_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect_err("stale key rejected");
    assert_eq!(error.code, connectrpc::ErrorCode::FailedPrecondition);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_library_requires_a_library_setting_id() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let before = harness.context.settings.libraries.len();
    let error = send_remove_library(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_SLEEP_COVER),
    )
    .await
    .expect_err("a non-library setting id is refused");
    assert_eq!(error.code, connectrpc::ErrorCode::InvalidArgument);
    assert_eq!(harness.context.settings.libraries.len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_library_paths_resolve_by_index_and_key() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::AlwaysOn);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let shared_dir = tempfile::tempdir().unwrap();
    send_add_library(
        &mut harness,
        library_payload("Alpha", shared_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect("add alpha");
    send_add_library(
        &mut harness,
        library_payload("Beta", shared_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect("add beta");
    let key = std::path::absolute(shared_dir.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let beta_index = harness.context.settings.libraries.len() - 1;

    let beta_id = proto::SettingId {
        kind: proto::SettingIdentityKind::SETTING_IDENTITY_KIND_LIBRARY_INFO.into(),
        index: Some(u32::try_from(beta_index).unwrap()),
        key: Some(key),
        ..Default::default()
    };
    send_update(
        &mut harness,
        beta_id,
        library_update("Second", shared_dir.path().to_str().unwrap(), ""),
    )
    .await
    .expect("update the library the id addresses");
    assert_eq!(
        harness.context.settings.libraries[beta_index].name,
        "Second"
    );
    assert_eq!(
        harness.context.settings.libraries[beta_index - 1].name,
        "Alpha"
    );
}
