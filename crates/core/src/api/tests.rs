//! API tests: listeners, bearer auth, settings round trips, and static files.

use super::public::{suspend, turn_off, turn_on};
use super::*;
use crate::device::DevicePaths;
use crate::device::test_harness::DeviceRuntimeHarness;
use crate::device::wifi::{Essid, NetworkInfo};
use crate::document::file_extension::FileExtension;
use crate::i18n::DEFAULT_LOCALE;
use crate::i18n::I18nDisplay;
use crate::settings::WifiMode;
use crate::view::EntryId;
use connectrpc::ConnectError;
use secrecy::ExposeSecret;
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
    })
}

#[cfg(feature = "emulator")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn menu_switch_binds_an_ephemeral_port() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(
            &Event::Select(EntryId::ToggleCompanion),
            hub,
            bus,
            context,
            runtime,
        )
    });
    let port = harness.context.api.local_addr().unwrap().port();
    assert_ne!(port, 0);
    assert_ne!(port, 80);
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
async fn switch_off_cancels_the_job_and_drops_the_lease() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    crate::poll_parts!(harness, |_hub, _bus, _rq, context, _runtime| {
        turn_off(context)
    });
    assert!(!holds_public_api_lease(&harness));
    assert!(!harness.context.api.is_on());
    tokio::time::sleep(Duration::from_millis(50)).await;
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
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspend_closes_the_listener_and_keeps_the_token() {
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
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::ApiResume, hub, bus, context, runtime)
    });
    assert_eq!(exposed_token(&harness), token);
    assert!(holds_public_api_lease(&harness));
    let again = harness.context.api.local_addr().unwrap();
    tokio::net::TcpStream::connect(again).await.unwrap();
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
    });
    rx.await.unwrap().expect("update applied");
    assert!(!harness.context.settings.sleep_cover);

    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::Update {
            id: sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_UNSPECIFIED),
            value: settings::choice_value("off", &[]),
            reply: tx,
        }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
    });
    assert!(rx.await.unwrap().is_err());
    assert_eq!(harness.context.settings.wifi, WifiMode::AlwaysOn);
    assert!(holds_public_api_lease(&harness));
    let _ = EntryId::ToggleCompanion;
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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

    let catalog = send_list(&mut harness).await;
    let listed = listed_setting(&catalog, lid(0));
    let value = listed.value.clone().into_option().unwrap_or_default();
    let proto::setting_value::Kind::Library(library) = value.kind.as_ref().expect("library kind")
    else {
        panic!("LibraryInfo listed as {:?}", value.kind);
    };
    assert_eq!(library.name, "Cadmus Source");
    let listed_path = std::path::Path::new(library.path.path.as_str());
    assert!(listed_path.is_absolute(), "{}", library.path.path);
    assert_eq!(
        listed_path,
        std::path::absolute(".").expect("current directory")
    );
    assert!(!library.remove);

    let added_dir = tempfile::tempdir().unwrap();
    let added_path = added_dir.path().to_string_lossy().into_owned();
    send_update(
        &mut harness,
        sid(proto::SettingIdentityKind::SETTING_IDENTITY_KIND_ADD_LIBRARY),
        library_update("Books", &added_path, "", false),
    )
    .await
    .expect("add library");
    assert_eq!(harness.context.settings.libraries.len(), 2);
    assert_eq!(harness.context.settings.libraries[1].name, "Books");
    assert_eq!(harness.context.settings.libraries[1].path, added_dir.path());

    let updated_dir = tempfile::tempdir().unwrap();
    let updated_path = updated_dir.path().to_string_lossy().into_owned();
    send_update(
        &mut harness,
        lid(0),
        library_update("Cadmus Source", &updated_path, "", false),
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
        library_update("Cadmus Source", file.path().to_str().unwrap(), "", false),
    )
    .await
    .expect_err("file path rejected");
    assert_eq!(
        harness.context.settings.libraries[0].path,
        updated_dir.path()
    );

    send_update(
        &mut harness,
        lid(1),
        library_update("Books", &added_path, "", true),
    )
    .await
    .expect("remove library");
    assert_eq!(harness.context.settings.libraries.len(), 1);
    assert_eq!(harness.context.settings.libraries[0].name, "Cadmus Source");
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

async fn send_list(harness: &mut DeviceRuntimeHarness) -> proto::SettingsCatalog {
    let (tx, rx) = oneshot::channel();
    harness
        .context
        .api
        .tx
        .send(Command::Settings(settings::Command::List { reply: tx }))
        .unwrap();
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
    });
    rx.await.unwrap()
}

fn listed_setting(catalog: &proto::SettingsCatalog, id: proto::SettingId) -> &proto::Setting {
    catalog
        .categories
        .iter()
        .flat_map(|category| &category.settings)
        .find(|setting| setting.id == id.clone().into())
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

fn library_update(name: &str, path: &str, finished: &str, remove: bool) -> proto::SettingValue {
    settings::library_value(
        name,
        path,
        proto::ChoiceValue {
            selected: finished.to_string(),
            ..Default::default()
        },
        remove,
    )
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
            assert!(rate.available.iter().any(|extension| extension == "epub"));
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
            assert!(!rate.available.iter().any(|extension| extension == "epub"));
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
    crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
        handle_event(&Event::Api, hub, bus, context, runtime)
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
                        extension: (*extension).to_string(),
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
    if cfg!(debug_assertions) {
        let listed = tokio::spawn(async move { http_status(addr, None).await });
        for _ in 0..40 {
            crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
                handle_event(&Event::Api, hub, bus, context, runtime)
            });
            if listed.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let listed = listed.await.unwrap();
        assert!(listed.contains("200"), "{listed}");
        return;
    }
    let missing = http_status(addr, None).await;
    let wrong = http_status(addr, Some("Bearer 0000")).await;
    assert!(missing.contains("401"), "missing token status: {missing}");
    assert!(wrong.contains("401"), "wrong token status: {wrong}");
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
    let listed = tokio::spawn(async move {
        http_exchange(
            addr,
            "POST /cadmus.api.shared.v1.Status/GetStatus HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        )
        .await
    });
    for _ in 0..40 {
        crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, context, runtime)
        });
        if listed.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let body = listed.await.unwrap();
    assert!(body.contains("200"), "{body}");
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
    let listed =
        tokio::spawn(async move { http_status(addr, Some(&format!("Bearer {token}"))).await });
    for _ in 0..40 {
        crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, context, runtime)
        });
        if listed.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let listed = listed.await.unwrap();
    assert!(listed.contains("200"), "{listed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openapi_json_is_served_without_a_token() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let spec = tokio::spawn(async move { http_get_complete(addr, "/openapi.json").await });
    for _ in 0..40 {
        crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, context, runtime)
        });
        if spec.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let response = spec.await.unwrap();
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_over_http2_serves_get_status() {
    let mut harness = DeviceRuntimeHarness::new().await;
    arm_wifi(&mut harness, WifiMode::Auto);
    start(&mut harness, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let addr = harness.context.api.local_addr().unwrap();
    let call = tokio::spawn(async move { grpc_status(addr).await });
    for _ in 0..40 {
        crate::poll_parts!(harness, |hub, bus, _rq, context, runtime| {
            handle_event(&Event::Api, hub, bus, context, runtime)
        });
        if call.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let (status, content_type, body_len) = call.await.unwrap();
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
    let (on_public, on_private) = tokio::join!(private_ping(public), private_ping(private));
    assert!(on_public.starts_with("HTTP/1.1 404"), "{on_public}");
    assert!(on_private.starts_with("HTTP/1.1 200"), "{on_private}");
}

/// `cadmus.api.internal.v1.Placeholder/Ping`, a no-op internal RPC.
async fn private_ping(addr: SocketAddr) -> String {
    http_exchange(
        addr,
        "POST /cadmus.api.internal.v1.Placeholder/Ping HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nConnect-Protocol-Version: 1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
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

async fn http_get_complete(addr: SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).to_string()
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
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf[..n]).to_string()
}
