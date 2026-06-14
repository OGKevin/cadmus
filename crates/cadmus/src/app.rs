use cadmus_core::anyhow::{Context as ResultExt, Error, format_err};
use cadmus_core::assets::open_documentation;
use cadmus_core::battery::{Battery, KoboBattery};
use cadmus_core::chrono::{Duration as ChronoDuration, Local, Timelike};
use cadmus_core::context::Context;
use cadmus_core::db::Database;
use cadmus_core::device::{CURRENT_DEVICE, FrontlightKind, Orientation};
use cadmus_core::document::sys_info_as_html;
use cadmus_core::font::Fonts;
use cadmus_core::framebuffer::{Framebuffer, KoboFramebuffer1, KoboFramebuffer2, UpdateMode};
use cadmus_core::frontlight::{
    Frontlight, NaturalFrontlight, PremixedFrontlight, StandardFrontlight,
};
use cadmus_core::geom::{DiagDir, Rectangle, Region};
use cadmus_core::gesture::{GestureEvent, gesture_events};
use cadmus_core::i18n;
use cadmus_core::input::{
    ButtonCode, ButtonStatus, DeviceEvent, PowerSource, VAL_PRESS, VAL_RELEASE,
};
use cadmus_core::input::{
    InputEvent, button_scheme_event, device_events, display_rotate_event, raw_events, usb_events,
};
use cadmus_core::library::Library;
use cadmus_core::lightsensor::{KoboLightSensor, LightSensor};
use cadmus_core::metadata::Info;
use cadmus_core::rtc::{AlarmType, EnsureAlarmOutcome, PastDueAction};
use cadmus_core::settings::versioned::SettingsManager;
use cadmus_core::settings::{
    ButtonScheme, IntermKind, IntermissionDisplay, LoggingSettings, RotationLock, Settings,
    StartupMode,
};
use cadmus_core::task::TaskManager;
use cadmus_core::version::{get_current_version, get_version};
use cadmus_core::view::calculator::Calculator;
use cadmus_core::view::common::{
    find_notification_mut, locate, locate_by_id, overlapping_rectangle, transfer_notifications,
};
use cadmus_core::view::common::{toggle_input_history_menu, toggle_keyboard_layout_menu};
use cadmus_core::view::dialog::Dialog;
use cadmus_core::view::dictionary::Dictionary as DictionaryApp;
use cadmus_core::view::frontlight::FrontlightWindow;
use cadmus_core::view::home::Home;
use cadmus_core::view::intermission::Intermission;
use cadmus_core::view::menu::{Menu, MenuKind};
use cadmus_core::view::notification::Notification;
use cadmus_core::view::ota::show_ota_view;
use cadmus_core::view::reader::Reader;
use cadmus_core::view::rotation_values::RotationValues;
use cadmus_core::view::settings_editor::SettingsEditor;
use cadmus_core::view::sketch::Sketch;
use cadmus_core::view::startup::StartupScreen;
use cadmus_core::view::touch_events::TouchEvents;
use cadmus_core::view::{
    AppCmd, Bus, EntryId, EntryKind, Event, Hub, NotificationEvent, RenderData, RenderQueue,
    UpdateData, View, ViewId,
};
use cadmus_core::view::{handle_event, process_render_queue, wait_for_all};
use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{Level, debug, error, info, warn};

pub const APP_NAME: &str = "Cadmus";
const FB_DEVICE: &str = "/dev/fb0";
const TOUCH_INPUTS: [&str; 5] = [
    "/dev/input/by-path/platform-2-0010-event",
    "/dev/input/by-path/platform-1-0038-event",
    "/dev/input/by-path/platform-1-0010-event",
    "/dev/input/by-path/platform-0-0010-event",
    "/dev/input/event1",
];
const BUTTON_INPUTS: [&str; 4] = [
    "/dev/input/by-path/platform-gpio-keys-event",
    "/dev/input/by-path/platform-ntx_event0-event",
    "/dev/input/by-path/platform-mxckpd-event",
    "/dev/input/event0",
];
const POWER_INPUTS: [&str; 3] = [
    "/dev/input/by-path/platform-bd71828-pwrkey.6.auto-event",
    "/dev/input/by-path/platform-bd71828-pwrkey.4.auto-event",
    "/dev/input/by-path/platform-bd71828-pwrkey-event",
];

const KOBO_UPDATE_BUNDLE: &str = "/mnt/onboard/.kobo/KoboRoot.tgz";

const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const BATTERY_REFRESH_INTERVAL: Duration = Duration::from_secs(299);
const AUTO_SUSPEND_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(15);
const PREPARE_SUSPEND_WAIT_DELAY: Duration = Duration::from_secs(3);

struct Task {
    id: TaskId,
    _chan: Receiver<()>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum TaskId {
    CheckBattery,
    PrepareSuspend,
    Exit,
    Suspend,
}

struct HistoryItem {
    view: Box<dyn View>,
    rotation: i8,
    monochrome: bool,
    dithered: bool,
}

#[allow(clippy::too_many_arguments)]
fn open_document(
    info: Box<Info>,
    view: &mut Box<dyn View>,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<UpdateData>,
    raw_sender: &Sender<InputEvent>,
    tx: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut Context,
) -> bool {
    let rotation = context.display.rotation;
    let dithered = context.fb.dithered();

    if let Some(reader_info) = info.reader.as_ref() {
        if let Some(n) = reader_info
            .rotation
            .map(|n| CURRENT_DEVICE.from_canonical(n))
            && CURRENT_DEVICE.orientation(n) != CURRENT_DEVICE.orientation(rotation)
        {
            wait_for_all(updating, context);
            if let Ok(dims) = context.fb.set_rotation(n) {
                raw_sender.send(display_rotate_event(n)).ok();
                context.display.rotation = n;
                context.display.dims = dims;
            }
        }
        context.fb.set_dithered(reader_info.dithered);
    } else {
        context.fb.set_dithered(
            info.file
                .kind
                .parse()
                .ok()
                .is_some_and(|kind| context.settings.reader.dithered_kinds.contains(&kind)),
        );
    }

    let path = info.file.path.clone();
    if let Some(r) = Reader::new(context.fb.rect(), *info, tx, context) {
        let mut next_view = Box::new(r) as Box<dyn View>;
        transfer_notifications(view.as_mut(), next_view.as_mut(), rq, context);
        if view.is::<Reader>() {
            *view = next_view;
        } else {
            let prev = std::mem::replace(view, next_view);
            history.push(HistoryItem {
                view: prev,
                rotation,
                monochrome: context.fb.monochrome(),
                dithered,
            });
        }
        true
    } else {
        if context.display.rotation != rotation
            && let Ok(dims) = context.fb.set_rotation(rotation)
        {
            raw_sender.send(display_rotate_event(rotation)).ok();
            context.display.rotation = rotation;
            context.display.dims = dims;
        }
        context.fb.set_dithered(dithered);
        warn!(
            path = %path.display(),
            library_home = %context.library.home.display(),
            "Reader::new returned None, dispatching Event::Invalid"
        );
        handle_event(view.as_mut(), &Event::Invalid(path), tx, bus, rq, context);
        false
    }
}

fn build_context(
    fb: Box<dyn Framebuffer>,
    settings: Settings,
    fonts: Fonts,
    database: Database,
) -> Result<Context, Error> {
    let mut settings = settings;

    if settings.libraries.is_empty() {
        return Err(format_err!("no libraries found"));
    }

    if settings.selected_library >= settings.libraries.len() {
        settings.selected_library = 0;
    }

    let library_settings = &settings.libraries[settings.selected_library];
    let library = Library::new(&library_settings.path, &database, &library_settings.name)?;

    let battery = Box::new(KoboBattery::new().context("can't create battery")?) as Box<dyn Battery>;

    let lightsensor = if CURRENT_DEVICE.has_lightsensor() {
        Box::new(KoboLightSensor::new().context("can't create light sensor")?)
            as Box<dyn LightSensor>
    } else {
        Box::new(0u16) as Box<dyn LightSensor>
    };

    let levels = settings.frontlight_levels;
    let frontlight = match CURRENT_DEVICE.frontlight_kind() {
        FrontlightKind::Standard => Box::new(
            StandardFrontlight::new(levels.intensity)
                .context("can't create standard frontlight")?,
        ) as Box<dyn Frontlight>,
        FrontlightKind::Natural => Box::new(
            NaturalFrontlight::new(levels.intensity, levels.warmth)
                .context("can't create natural frontlight")?,
        ) as Box<dyn Frontlight>,
        FrontlightKind::Premixed => Box::new(
            PremixedFrontlight::new(levels.intensity, levels.warmth)
                .context("can't create premixed frontlight")?,
        ) as Box<dyn Frontlight>,
    };

    Ok(Context::new(
        fb,
        library,
        database,
        settings,
        fonts,
        battery,
        frontlight,
        lightsensor,
    ))
}

fn schedule_task(
    id: TaskId,
    event: Event,
    delay: Duration,
    hub: &Sender<Event>,
    tasks: &mut Vec<Task>,
) {
    let (ty, ry) = mpsc::channel();
    let hub2 = hub.clone();
    tasks.retain(|task| task.id != id);
    tasks.push(Task { id, _chan: ry });
    thread::spawn(move || {
        thread::sleep(delay);
        if ty.send(()).is_ok() {
            hub2.send(event).ok();
        }
    });
}

fn resume(
    id: TaskId,
    tasks: &mut Vec<Task>,
    view: &mut dyn View,
    hub: &Sender<Event>,
    rq: &mut RenderQueue,
    context: &mut Context,
) {
    debug!(task = ?id, "resume called");
    if id == TaskId::Suspend {
        tasks.retain(|task| task.id != TaskId::Suspend);
        context.set_frontlight(context.settings.frontlight);
        if context.settings.wifi {
            if let Ok(wifi) = CURRENT_DEVICE.wifi_manager() {
                thread::spawn(move || {
                    if let Err(e) = wifi.enable() {
                        tracing::error!(error = %e, "Failed to enable WiFi on resume");
                    }
                });
            }
        }
        if let Some(alarm_manager) = context.alarm_manager.as_mut() {
            for alarm in AlarmType::alarms_to_cancel_after_resume() {
                if let Err(e) = alarm_manager.cancel_alarm(alarm) {
                    error!(error = ?e, alarm = ?alarm, "failed to cancel alarm after resume")
                }
            }
        }
    }
    if id == TaskId::Suspend || id == TaskId::PrepareSuspend {
        tasks.retain(|task| task.id != TaskId::PrepareSuspend);

        if tracing::enabled!(Level::DEBUG) {
            let intermission_count = view
                .children()
                .iter()
                .filter(|c| c.is::<Intermission>())
                .count();
            debug!(intermission_count, "intermission views before cleanup");
        }

        if let Some(index) = locate::<Intermission>(view) {
            let rect = *view.child(index).rect();
            view.children_mut().remove(index);
            debug!("intermission view removed, queuing expose");
            rq.add(RenderData::expose(rect, UpdateMode::Full));
        } else {
            warn!("resume called but no intermission view found to remove");
        }

        hub.send(Event::ClockTick).ok();
        hub.send(Event::BatteryTick).ok();
    }
}

fn power_off(
    view: &mut dyn View,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<UpdateData>,
    context: &mut Context,
) {
    let (tx, _rx) = mpsc::channel();
    view.handle_event(
        &Event::Back,
        &tx,
        &mut VecDeque::new(),
        &mut RenderQueue::new(),
        context,
    );
    while let Some(mut item) = history.pop() {
        item.view.handle_event(
            &Event::Back,
            &tx,
            &mut VecDeque::new(),
            &mut RenderQueue::new(),
            context,
        );
    }
    let interm = Intermission::new(context.fb.rect(), IntermKind::PowerOff, context);
    wait_for_all(updating, context);
    interm.render(context.fb.as_mut(), *interm.rect(), &mut context.fonts);
    context.fb.update(interm.rect(), UpdateMode::Full).ok();
}

fn initiate_suspend(
    view: &mut dyn View,
    hub: &Sender<Event>,
    bus: &mut VecDeque<Event>,
    rq: &mut RenderQueue,
    tasks: &mut Vec<Task>,
    context: &mut Context,
) {
    debug!("initiating suspend");
    view.handle_event(&Event::Suspend, hub, bus, rq, context);
    let interm = Intermission::new(context.fb.rect(), IntermKind::Suspend, context);
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    schedule_task(
        TaskId::PrepareSuspend,
        Event::PrepareSuspend,
        PREPARE_SUSPEND_WAIT_DELAY,
        hub,
        tasks,
    );
    view.children_mut().push(Box::new(interm) as Box<dyn View>);
    debug!("suspend intermission pushed, PrepareSuspend scheduled");
}

fn set_wifi(enable: bool, context: &mut Context) {
    if context.settings.wifi == enable {
        return;
    }
    context.settings.wifi = enable;
    if let Ok(wifi) = CURRENT_DEVICE.wifi_manager() {
        if context.settings.wifi {
            thread::spawn(move || {
                if let Err(e) = wifi.enable() {
                    tracing::error!(error = %e, "Failed to enable WiFi");
                }
            });
        } else {
            thread::spawn(move || {
                if let Err(e) = wifi.disable() {
                    tracing::error!(error = %e, "Failed to disable WiFi");
                }
            });
            context.online = false;
        }
    }
}

/// Closes the database, flushes library state, saves settings, and disables
/// power-consuming features in preparation for USB sharing.
///
/// The database is closed to release file handles before `/mnt/onboard` is
/// unmounted during USB sharing. The library is flushed and settings are saved
/// to persist state before the share begins. Frontlight and wifi are disabled
/// to conserve power during the share session.
#[inline]
#[allow(clippy::too_many_arguments)] // This requires a greater device architecture refactor
fn prepare_share_for_usb(
    view: &mut Box<dyn View>,
    history: &mut Vec<HistoryItem>,
    tasks: &mut Vec<Task>,
    manager: &SettingsManager,
    context: &mut Context,
    raw_sender: &Sender<InputEvent>,
    updating: &mut Vec<UpdateData>,
    bus: &mut VecDeque<Event>,
    tx: &Sender<Event>,
    rq: &mut RenderQueue,
) {
    tasks.clear();
    view.handle_event(&Event::Back, tx, bus, rq, context);
    while let Some(mut item) = history.pop() {
        item.view.handle_event(&Event::Back, tx, bus, rq, context);
        if item.rotation != context.display.rotation {
            wait_for_all(updating, context);
            if let Ok(dims) = context.fb.set_rotation(item.rotation) {
                raw_sender.send(display_rotate_event(item.rotation)).ok();
                context.display.rotation = item.rotation;
                context.display.dims = dims;
            }
        }
        *view = item.view;
    }
    manager
        .save(&context.settings)
        .map_err(|e| error!("Can't save settings: {:#}.", e))
        .ok();
    context.database.close();

    if context.settings.frontlight {
        context.set_frontlight(false);
    }
    #[cfg(not(feature = "test"))]
    if context.settings.wifi {
        if let Ok(wifi) = CURRENT_DEVICE.wifi_manager() {
            if let Err(e) = wifi.disable() {
                tracing::error!(error = %e, "Failed to disable WiFi for USB share");
            }
        }
        context.online = false;
    }

    let interm = Intermission::new(context.fb.rect(), IntermKind::Share, context);
    rq.add(RenderData::new(
        interm.id(),
        *interm.rect(),
        UpdateMode::Full,
    ));
    view.children_mut().push(Box::new(interm) as Box<dyn View>);
    tx.send(Event::Share).ok();
}

/// Changes the working directory to `/tmp` and enables USB sharing.
///
/// The `/mnt/onboard` filesystem is unmounted when USB sharing starts.
/// Logging is redirected to `/tmp/cadmus-logs` before unmounting so log
/// writes during the share window do not fail on an unmounted filesystem.
/// Setting the working directory to `/tmp` ensures it remains valid throughout
/// the share session, preventing file operation failures.
///
/// Sets `context.shared = true` only when `enable()` succeeds. On failure,
/// shows a transient notification and schedules a device reboot after 3 seconds.
/// The share screen remains visible during the reboot window.
///
/// ## Logging and Corruption Risk
///
/// If log redirection fails, shows a transient notification and schedules an app restart after 3
/// seconds. This is done because if log redirection fails and the app tries to write logs while
/// the mount is unmounted, it can lead to corruption. See [#246](https://github.com/OGKevin/cadmus/issues/246).
#[inline]
#[cfg_attr(feature = "tracing", tracing::instrument(skip(tx, tasks, context)))]
fn start_usb_share(tx: &Sender<Event>, tasks: &mut Vec<Task>, context: &mut Context) {
    if let Err(e) = cadmus_core::logging::redirect_log_to_dir(
        std::path::Path::new("/tmp/cadmus-logs"),
        &context.settings.logging,
    ) {
        eprintln!("Failed to redirect logging to /tmp: {e}");

        tx.send(Event::Notification(NotificationEvent::Show(
            "Failed to start USB session".to_string(),
        )))
        .ok();
        schedule_task(
            TaskId::Exit,
            Event::Select(EntryId::Restart),
            Duration::from_secs(3),
            tx,
            tasks,
        );

        return;
    }

    match CURRENT_DEVICE.usb_manager() {
        Ok(usb_manager) => match usb_manager.enable() {
            Ok(()) => {
                context.shared = true;
                if let Err(e) = env::set_current_dir("/tmp") {
                    error!(error = %e, "failed to set working directory to /tmp before USB share");
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to enable USB sharing");
                tx.send(Event::Notification(NotificationEvent::Show(
                    "Failed to start USB session".to_string(),
                )))
                .ok();
                schedule_task(
                    TaskId::Exit,
                    Event::Select(EntryId::Reboot),
                    Duration::from_secs(3),
                    tx,
                    tasks,
                );
            }
        },
        Err(e) => {
            error!(error = %e, "Failed to create USB manager");
            tx.send(Event::Notification(NotificationEvent::Show(
                "Failed to start USB session".to_string(),
            )))
            .ok();
            schedule_task(
                TaskId::Exit,
                Event::Select(EntryId::Restart),
                Duration::from_secs(3),
                tx,
                tasks,
            );
        }
    }
}

/// Disables USB sharing, restores the working directory, and triggers
/// reboot or restart.
///
/// Disables USB mass storage mode. After the filesystem is remounted, logging
/// is restored to the configured path before the working directory is changed
/// so the log path resolves against the startup CWD rather than `/tmp`.
///
/// `context.shared` is not set back to false, as the app is going to be
/// restarted anyway. Leaving this as true helps the exit logic to not save
/// settings after USB share, ensuring that manual edits during the share
/// are not lost.
///
/// Finally, checks for `KoboRoot.tgz` to determine whether to reboot
/// (update pending) or restart the app.
#[inline]
fn handle_usb_unshare(
    startup_cwd: &Option<PathBuf>,
    logging_settings: &LoggingSettings,
    tx: &Sender<Event>,
) {
    info!("USB unplugged after sharing; disabling USB mass storage");

    match CURRENT_DEVICE.usb_manager() {
        Ok(usb_manager) => match usb_manager.disable() {
            Ok(()) => {
                info!("USB mass storage disabled successfully");
                if startup_cwd.is_some() {
                    let log_dir = CURRENT_DEVICE.data_path(&logging_settings.directory);
                    if let Err(e) =
                        cadmus_core::logging::redirect_log_to_dir(&log_dir, logging_settings)
                    {
                        eprintln!("Failed to restore logging after USB unshare: {e}");
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to disable USB sharing, triggering reboot");
                tx.send(Event::Select(EntryId::Reboot)).ok();
                return;
            }
        },
        Err(e) => {
            error!(error = %e, "Failed to create USB manager, triggering reboot");
            tx.send(Event::Select(EntryId::Reboot)).ok();
            return;
        }
    }

    if let Some(cwd) = startup_cwd.as_ref() {
        if let Err(e) = env::set_current_dir(cwd) {
            error!(error = %e, original_cwd = %cwd.display(), "failed to restore working directory after USB share");
        }
    }

    let update_bundle_exists = Path::new(KOBO_UPDATE_BUNDLE).exists();
    info!(update_bundle_exists, "filesystem state after USB disable");

    if update_bundle_exists {
        info!("KoboRoot.tgz detected; triggering reboot");
        tx.send(Event::Select(EntryId::Reboot)).ok();
    } else {
        info!("triggering app restart");
        tx.send(Event::Select(EntryId::Restart)).ok();
    }
}

#[derive(PartialEq)]
enum ExitStatus {
    Quit,
    Restart,
    Reboot,
    PowerOff,
}

pub fn run() -> Result<(), Error> {
    let mut inactive_since = Instant::now();
    let mut exit_status = ExitStatus::Quit;

    let mut fb: Box<dyn Framebuffer> = if CURRENT_DEVICE.mark() != 8 {
        Box::new(KoboFramebuffer1::new(FB_DEVICE).context("can't create framebuffer")?)
    } else {
        Box::new(KoboFramebuffer2::new(FB_DEVICE).context("can't create framebuffer")?)
    };

    let initial_rotation = CURRENT_DEVICE.transformed_rotation(fb.rotation());
    let startup_rotation = CURRENT_DEVICE.startup_rotation();
    if !CURRENT_DEVICE.has_gyroscope() && initial_rotation != startup_rotation {
        fb.set_rotation(startup_rotation).ok();
    }

    let manager = SettingsManager::new(CURRENT_DEVICE.data_dir(), get_current_version());
    let settings = manager.load();

    cadmus_core::crypto::init_crypto_provider();

    if let Err(e) = cadmus_core::logging::init_logging(
        &settings.logging,
        CURRENT_DEVICE.data_path(&settings.logging.directory),
    ) {
        eprintln!("Warning: Failed to initialize logging: {:#}", e);
        eprintln!("Continuing without logging...");
    }

    cadmus_core::document::log_mupdf_features();

    #[cfg(feature = "profiling")]
    if let Err(e) = cadmus_core::telemetry::profiling::init_profiling(
        settings.logging.pyroscope_endpoint.as_deref(),
    ) {
        tracing::warn!(error = %e, "Failed to initialize profiling");
    }

    i18n::init(settings.locale.as_ref());

    let startup_cwd = env::current_dir().ok();
    info!(cwd = ?startup_cwd, "startup diagnostics");
    CURRENT_DEVICE.clean_tmp_dir();

    match CURRENT_DEVICE.power_manager() {
        Ok(power) => {
            if let Err(e) = power.init_cores() {
                tracing::error!(error = %e, "Failed to initialize CPU cores");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to retrieve power manager");
        }
    }

    let mut fonts = Fonts::load().context("can't load fonts")?;
    let database = Database::new(CURRENT_DEVICE.resolve_db_path())
        .map_err(|e| {
            error!(error = %e, "can't open database");
            e
        })
        .context("can't open database")?;

    let startup = StartupScreen::new(fb.rect());
    startup.render(fb.as_mut(), *startup.rect(), &mut fonts);
    fb.update(startup.rect(), UpdateMode::Full).ok();

    let db = database.clone();
    if let Err(e) = db.migrate() {
        error!(error = %e, "migrations failed");
    }

    let mut context =
        build_context(fb, settings, fonts, database).context("can't build context")?;

    context.plugged = context.battery.status().is_ok_and(|v| v[0].is_wired());

    context.load_dictionaries();
    context.load_keyboard_layouts();

    let mut paths = Vec::new();
    for ti in &TOUCH_INPUTS {
        if Path::new(ti).exists() {
            paths.push(ti.to_string());
            break;
        }
    }
    for bi in &BUTTON_INPUTS {
        if Path::new(bi).exists() {
            paths.push(bi.to_string());
            break;
        }
    }
    for pi in &POWER_INPUTS {
        if Path::new(pi).exists() {
            paths.push(pi.to_string());
            break;
        }
    }

    let (raw_sender, raw_receiver) = raw_events(paths);
    let touch_screen = gesture_events(device_events(
        raw_receiver,
        context.display,
        context.settings.button_scheme,
    ));
    let usb_port = usb_events();

    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();

    thread::spawn(move || {
        while let Ok(evt) = touch_screen.recv() {
            tx2.send(evt).ok();
        }
    });

    let tx3 = tx.clone();
    thread::spawn(move || {
        while let Ok(evt) = usb_port.recv() {
            tx3.send(Event::Device(evt)).ok();
        }
    });

    let tx4 = tx.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(CLOCK_REFRESH_INTERVAL);
            tx4.send(Event::ClockTick).ok();
        }
    });

    let tx5 = tx.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(BATTERY_REFRESH_INTERVAL);
            tx5.send(Event::BatteryTick).ok();
        }
    });

    if context.settings.auto_suspend > 0.0 {
        let tx6 = tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(AUTO_SUSPEND_REFRESH_INTERVAL);
                tx6.send(Event::MightSuspend).ok();
            }
        });
    }

    context.fb.set_inverted(context.settings.inverted);

    {
        debug!("starting startup wifi management");

        match CURRENT_DEVICE.wifi_manager() {
            Ok(wifi) => {
                let wifi_enabled = context.settings.wifi;
                debug!(wifi_enabled, "wifi status");

                thread::spawn(move || {
                    if wifi_enabled {
                        if let Err(e) = wifi.enable() {
                            tracing::error!(error = %e, "Failed to enable WiFi on startup");
                        }
                    } else {
                        if let Err(e) = wifi.disable() {
                            tracing::error!(error = %e, "Failed to disable WiFi on startup");
                        }
                    }
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to create wifi manager")
            }
        }
    }

    context.set_frontlight(context.settings.frontlight);

    let mut tasks: Vec<Task> = Vec::new();
    let mut background_tasks = TaskManager::new();

    cadmus_core::task::register_startup_tasks(
        &mut background_tasks,
        tx.clone(),
        &context.settings,
        &context.database,
    );

    let mut history: Vec<HistoryItem> = Vec::new();
    let mut rq = RenderQueue::new();
    let mut view: Box<dyn View> = Box::new(Home::new(context.fb.rect(), &mut rq, &mut context)?);

    let mut updating = Vec::new();

    let version = get_version();
    info!(
        "{} {} {} is running on a Kobo {}.",
        APP_NAME,
        version.git(),
        version
            .pull_request()
            .map(|pull_request| pull_request.as_str())
            .unwrap_or(""),
        CURRENT_DEVICE.model
    );
    info!(
        "The framebuffer resolution is {} by {}.",
        context.fb.rect().width(),
        context.fb.rect().height()
    );

    let mut bus = VecDeque::with_capacity(4);

    if context.settings.startup_mode == StartupMode::LastFile
        && let Some(info) = context.library.most_recently_opened_reading_book()
    {
        open_document(
            Box::new(info),
            &mut view,
            &mut history,
            &mut updating,
            &raw_sender,
            &tx,
            &mut bus,
            &mut rq,
            &mut context,
        );
    }

    schedule_task(
        TaskId::CheckBattery,
        Event::CheckBattery,
        BATTERY_REFRESH_INTERVAL,
        &tx,
        &mut tasks,
    );
    tx.send(Event::WakeUp).ok();

    while let Ok(evt) = rx.recv() {
        #[cfg(feature = "tracing")]
        let span = tracing::trace_span!("main-event-loop", event = ?evt);
        #[cfg(feature = "tracing")]
        let _enter = span.enter();
        #[cfg(feature = "tracing")]
        tracing::trace!(event = ?evt, "handling event");

        background_tasks.handle_event(&evt, &tx, &context);

        match evt {
            Event::Device(de) => match de {
                DeviceEvent::Button {
                    code: ButtonCode::Power,
                    status: ButtonStatus::Released,
                    ..
                } => {
                    if context.shared || context.covered {
                        continue;
                    }

                    if tasks.iter().any(|task| task.id == TaskId::PrepareSuspend) {
                        debug!("power button: resuming from PrepareSuspend");
                        resume(
                            TaskId::PrepareSuspend,
                            &mut tasks,
                            view.as_mut(),
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                    } else if tasks.iter().any(|task| task.id == TaskId::Suspend) {
                        debug!("power button: resuming from Suspend");
                        resume(
                            TaskId::Suspend,
                            &mut tasks,
                            view.as_mut(),
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                    } else {
                        debug!("power button: initiating new suspend");
                        initiate_suspend(
                            view.as_mut(),
                            &tx,
                            &mut bus,
                            &mut rq,
                            &mut tasks,
                            &mut context,
                        );
                    }
                }
                DeviceEvent::Button {
                    code: ButtonCode::Light,
                    status: ButtonStatus::Pressed,
                    ..
                } => {
                    tx.send(Event::ToggleFrontlight).ok();
                }
                DeviceEvent::CoverOn => {
                    if context.covered {
                        continue;
                    }

                    context.covered = true;

                    if !context.settings.sleep_cover
                        || context.shared
                        || tasks.iter().any(|task| {
                            task.id == TaskId::PrepareSuspend || task.id == TaskId::Suspend
                        })
                    {
                        continue;
                    }

                    initiate_suspend(
                        view.as_mut(),
                        &tx,
                        &mut bus,
                        &mut rq,
                        &mut tasks,
                        &mut context,
                    );
                }
                DeviceEvent::CoverOff => {
                    if !context.covered {
                        continue;
                    }

                    context.covered = false;

                    if context.shared || !context.settings.sleep_cover {
                        continue;
                    }

                    if tasks.iter().any(|task| task.id == TaskId::PrepareSuspend) {
                        resume(
                            TaskId::PrepareSuspend,
                            &mut tasks,
                            view.as_mut(),
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                    } else if tasks.iter().any(|task| task.id == TaskId::Suspend) {
                        resume(
                            TaskId::Suspend,
                            &mut tasks,
                            view.as_mut(),
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                    }
                }
                DeviceEvent::NetUp => {
                    if tasks
                        .iter()
                        .any(|task| task.id == TaskId::PrepareSuspend || task.id == TaskId::Suspend)
                        || context.online
                    {
                        continue;
                    }
                    let ip = Command::new("scripts/ip.sh")
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
                        .unwrap_or_default();
                    let essid = Command::new("scripts/essid.sh")
                        .output()
                        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
                        .unwrap_or_default();
                    let notif = Notification::new(
                        None,
                        format!("Network is up ({}, {}).", ip, essid),
                        false,
                        &tx,
                        &mut rq,
                        &mut context,
                    );
                    context.online = true;
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                    if view.is::<Home>() {
                        view.handle_event(&evt, &tx, &mut bus, &mut rq, &mut context);
                    } else if let Some(entry) =
                        history.get_mut(0).filter(|entry| entry.view.is::<Home>())
                    {
                        let (tx, _rx) = mpsc::channel();
                        entry.view.handle_event(
                            &evt,
                            &tx,
                            &mut VecDeque::new(),
                            &mut RenderQueue::new(),
                            &mut context,
                        );
                    }
                }
                DeviceEvent::Plug(power_source) => {
                    if context.plugged {
                        continue;
                    }

                    context.plugged = true;

                    tasks.retain(|task| task.id != TaskId::CheckBattery);

                    if context.covered {
                        continue;
                    }

                    match power_source {
                        PowerSource::Wall => {
                            if tasks.iter().any(|task| task.id == TaskId::Suspend) {
                                continue;
                            }
                        }
                        PowerSource::Host => {
                            if tasks.iter().any(|task| task.id == TaskId::PrepareSuspend) {
                                resume(
                                    TaskId::PrepareSuspend,
                                    &mut tasks,
                                    view.as_mut(),
                                    &tx,
                                    &mut rq,
                                    &mut context,
                                );
                            } else if tasks.iter().any(|task| task.id == TaskId::Suspend) {
                                resume(
                                    TaskId::Suspend,
                                    &mut tasks,
                                    view.as_mut(),
                                    &tx,
                                    &mut rq,
                                    &mut context,
                                );
                            }

                            if context.settings.auto_share {
                                tx.send(Event::PrepareShare).ok();
                            } else {
                                let dialog = Dialog::builder(
                                    ViewId::ShareDialog,
                                    "Share storage via USB?".to_string(),
                                )
                                .add_button("Cancel", Event::Close(ViewId::ShareDialog))
                                .add_button("Share", Event::PrepareShare)
                                .build(&mut context);
                                rq.add(RenderData::new(
                                    dialog.id(),
                                    *dialog.rect(),
                                    UpdateMode::Gui,
                                ));
                                view.children_mut().push(Box::new(dialog) as Box<dyn View>);
                            }

                            inactive_since = Instant::now();
                        }
                    }

                    tx.send(Event::BatteryTick).ok();
                }
                DeviceEvent::Unplug(..) => {
                    if !context.plugged {
                        continue;
                    }

                    if context.shared {
                        handle_usb_unshare(&startup_cwd, &context.settings.logging, &tx);
                    } else {
                        context.plugged = false;
                        schedule_task(
                            TaskId::CheckBattery,
                            Event::CheckBattery,
                            BATTERY_REFRESH_INTERVAL,
                            &tx,
                            &mut tasks,
                        );
                        if tasks.iter().any(|task| task.id == TaskId::Suspend) {
                            if !context.covered {
                                resume(
                                    TaskId::Suspend,
                                    &mut tasks,
                                    view.as_mut(),
                                    &tx,
                                    &mut rq,
                                    &mut context,
                                );
                            }
                        } else {
                            tx.send(Event::BatteryTick).ok();
                        }
                    }
                }
                DeviceEvent::RotateScreen(n) => {
                    if context.shared
                        || tasks.iter().any(|task| {
                            task.id == TaskId::PrepareSuspend || task.id == TaskId::Suspend
                        })
                    {
                        continue;
                    }

                    if view.is::<RotationValues>() {
                        debug!("Gyro rotation: {}", n);
                    }

                    if let Some(rotation_lock) = context.settings.rotation_lock {
                        let orientation = CURRENT_DEVICE.orientation(n);
                        if rotation_lock == RotationLock::Current
                            || (rotation_lock == RotationLock::Portrait
                                && orientation == Orientation::Landscape)
                            || (rotation_lock == RotationLock::Landscape
                                && orientation == Orientation::Portrait)
                        {
                            continue;
                        }
                    }

                    tx.send(Event::Select(EntryId::Rotate(n))).ok();
                }
                DeviceEvent::UserActivity if context.settings.auto_suspend > 0.0 => {
                    inactive_since = Instant::now();
                }
                _ => {
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
                }
            },
            Event::CheckBattery => {
                schedule_task(
                    TaskId::CheckBattery,
                    Event::CheckBattery,
                    BATTERY_REFRESH_INTERVAL,
                    &tx,
                    &mut tasks,
                );
                if tasks
                    .iter()
                    .any(|task| task.id == TaskId::PrepareSuspend || task.id == TaskId::Suspend)
                {
                    continue;
                }
                if let Ok(v) = context.battery.capacity().map(|v| v[0]) {
                    if v < context.settings.battery.power_off {
                        power_off(view.as_mut(), &mut history, &mut updating, &mut context);
                        exit_status = ExitStatus::PowerOff;
                        break;
                    } else if v < context.settings.battery.warn {
                        let notif = Notification::new(
                            None,
                            "The battery capacity is getting low.".to_string(),
                            false,
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                        view.children_mut().push(Box::new(notif) as Box<dyn View>);
                    }
                }
            }
            Event::PrepareSuspend => {
                tasks.retain(|task| task.id != TaskId::PrepareSuspend);
                wait_for_all(&mut updating, &mut context);
                manager
                    .save(&context.settings)
                    .map_err(|e| error!("Can't save settings: {:#}.", e))
                    .ok();

                if context.settings.frontlight {
                    context.settings.frontlight_levels = context.frontlight.levels();
                    if let Err(error) = context.frontlight.turn_off() {
                        tracing::error!(error = %error, "failed to turn off frontlight for suspend");
                    }
                }
                if context.settings.wifi {
                    if let Ok(wifi) = CURRENT_DEVICE.wifi_manager() {
                        if let Err(e) = wifi.disable() {
                            tracing::error!(error = %e, "Failed to disable WiFi on suspend");
                        }
                    }
                    context.online = false;
                }
                // https://github.com/koreader/koreader/commit/71afe36
                schedule_task(
                    TaskId::Suspend,
                    Event::Suspend,
                    SUSPEND_WAIT_DELAY,
                    &tx,
                    &mut tasks,
                );
            }
            Event::Suspend => {
                if let Some(alarm_manager) = context.alarm_manager.as_mut() {
                    if context.settings.auto_power_off > 0.0 {
                        let duration = ChronoDuration::seconds(
                            (context.settings.auto_power_off * 86_400.0) as i64,
                        );
                        match alarm_manager.ensure_scheduled(
                            AlarmType::AutoPowerOff,
                            duration,
                            PastDueAction::Cancel,
                        ) {
                            Ok(EnsureAlarmOutcome::PastDue) => {
                                info!("AutoPowerOff alarm is past due, powering off");
                                power_off(view.as_mut(), &mut history, &mut updating, &mut context);
                                exit_status = ExitStatus::PowerOff;
                                break;
                            }
                            Ok(_) => {}
                            Err(e) => error!(error = %e, "Can't schedule auto power off alarm"),
                        }
                    }
                    if context.settings.intermissions[IntermKind::Suspend]
                        == IntermissionDisplay::Calendar
                    {
                        let now = Local::now();
                        let seconds_into_current_5min =
                            (now.minute() as i64 % 5) * 60 + now.second() as i64;
                        // +1 to ensure we're always past the 5m clock mark
                        let seconds_until_next_5min = 300 - seconds_into_current_5min + 1;
                        alarm_manager
                            .ensure_scheduled(
                                AlarmType::CalendarUpdate,
                                ChronoDuration::seconds(seconds_until_next_5min),
                                PastDueAction::Reschedule,
                            )
                            .map_err(|e| error!(error = %e, "Can't schedule calendar update alarm"))
                            .ok();
                    }
                }
                let before = Local::now();
                info!(
                    "{}",
                    before.format("Went to sleep on %B %-d, %Y at %H:%M:%S.")
                );
                match CURRENT_DEVICE.power_manager() {
                    Ok(power) => {
                        if let Err(e) = power.suspend() {
                            tracing::error!(error = %e, "Failed to suspend device");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "power_manager() initialization failed for suspend");
                    }
                }
                let after = Local::now();
                info!("{}", after.format("Woke up on %B %-d, %Y at %H:%M:%S."));
                match CURRENT_DEVICE.power_manager() {
                    Ok(power) => {
                        if let Err(e) = power.resume() {
                            tracing::error!(error = %e, "Failed to resume device");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "power_manager() initialization failed for resume");
                    }
                }
                inactive_since = Instant::now();
                let pending_task_ids: Vec<_> = tasks.iter().map(|t| t.id).collect();
                debug!(pending_tasks = ?pending_task_ids, "task state after wake");
                // If the wake is legitimate, the task will be cancelled by `resume`.
                schedule_task(
                    TaskId::Suspend,
                    Event::Suspend,
                    SUSPEND_WAIT_DELAY,
                    &tx,
                    &mut tasks,
                );
                if let Some(alarm_manager) = context.alarm_manager.as_mut() {
                    match alarm_manager.check_fired_alarms(before.to_utc(), after.to_utc()) {
                        Ok(fired_alarms) => {
                            info!(alarms = ?fired_alarms, "Checked fired alarms after wake");
                            if fired_alarms.contains(&AlarmType::AutoPowerOff) {
                                power_off(view.as_mut(), &mut history, &mut updating, &mut context);
                                exit_status = ExitStatus::PowerOff;
                                break;
                            }
                            if fired_alarms.contains(&AlarmType::CalendarUpdate)
                                && context.settings.intermissions[IntermKind::Suspend]
                                    == IntermissionDisplay::Calendar
                            {
                                debug!(
                                    "CalendarUpdate alarm fired; refreshing calendar intermission"
                                );
                                if let Some(index) = locate::<Intermission>(view.as_mut()) {
                                    view.children_mut().remove(index);
                                    debug!("old calendar intermission removed");
                                }
                                let interm = Intermission::new(
                                    context.fb.rect(),
                                    IntermKind::Suspend,
                                    &context,
                                );
                                rq.add(RenderData::new(
                                    interm.id(),
                                    *interm.rect(),
                                    UpdateMode::Full,
                                ));
                                view.children_mut().push(Box::new(interm) as Box<dyn View>);
                            }
                        }
                        Err(e) => {
                            error!(error = %e, "Error checking fired alarms");
                        }
                    }
                }
            }
            Event::PrepareShare => {
                if context.shared {
                    continue;
                }

                prepare_share_for_usb(
                    &mut view,
                    &mut history,
                    &mut tasks,
                    &manager,
                    &mut context,
                    &raw_sender,
                    &mut updating,
                    &mut bus,
                    &tx,
                    &mut rq,
                );
            }
            Event::Share => {
                if context.shared {
                    continue;
                }

                start_usb_share(&tx, &mut tasks, &mut context);
            }
            Event::Gesture(ge) => match ge {
                GestureEvent::HoldButtonLong(ButtonCode::Power) => {
                    power_off(view.as_mut(), &mut history, &mut updating, &mut context);
                    exit_status = ExitStatus::PowerOff;
                    break;
                }
                GestureEvent::MultiTap(mut points) => {
                    if points[0].x > points[1].x {
                        points.swap(0, 1);
                    }
                    let rect = context.fb.rect();
                    let r1 = Region::from_point(
                        points[0],
                        rect,
                        context.settings.reader.strip_width,
                        context.settings.reader.corner_width,
                    );
                    let r2 = Region::from_point(
                        points[1],
                        rect,
                        context.settings.reader.strip_width,
                        context.settings.reader.corner_width,
                    );
                    match (r1, r2) {
                        (
                            Region::Corner(DiagDir::SouthWest),
                            Region::Corner(DiagDir::NorthEast),
                        ) => {
                            rq.add(RenderData::new(
                                view.id(),
                                context.fb.rect(),
                                UpdateMode::Full,
                            ));
                        }
                        (
                            Region::Corner(DiagDir::NorthWest),
                            Region::Corner(DiagDir::SouthEast),
                        ) => {
                            tx.send(Event::Select(EntryId::TakeScreenshot)).ok();
                        }
                        _ => (),
                    }
                }
                _ => {
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
                }
            },
            Event::ToggleFrontlight => {
                context.set_frontlight(!context.settings.frontlight);
                view.handle_event(
                    &Event::ToggleFrontlight,
                    &tx,
                    &mut bus,
                    &mut rq,
                    &mut context,
                );
            }
            Event::SetFrontlightLevels(levels) => {
                #[cfg(feature = "kobo")]
                if let Err(e) = background_tasks.stop(&cadmus_core::task::TaskId::AutoFrontlight)
                    && !matches!(
                        e,
                        cadmus_core::task::TaskError::NotRunning(
                            cadmus_core::task::TaskId::AutoFrontlight
                        )
                    )
                {
                    tracing::warn!(error = %e, "failed to stop auto_frontlight task after manual adjustment");
                }

                if let Err(error) = context.frontlight.set_intensity(levels.intensity) {
                    tracing::error!(error = %error, "failed to set frontlight intensity");
                }
                if let Err(error) = context.frontlight.set_warmth(levels.warmth) {
                    tracing::error!(error = %error, "failed to set frontlight warmth");
                }
                context.settings.frontlight_levels = levels;
            }
            // TODO(OGKevin): once https://github.com/OGKevin/cadmus/issues/582 is done
            //                this shall be refactored accordingly.
            Event::UpdateAutoFrontlight => {
                if context.settings.auto_frontlight && context.settings.frontlight {
                    if let Some(coords) =
                        cadmus_core::settings::resolve_coordinates(&context.settings)
                    {
                        let night_brightness = context
                            .settings
                            .auto_frontlight_night_brightness
                            .unwrap_or_default();
                        let current_intensity = context.frontlight.levels().intensity;
                        let levels = cadmus_core::frontlight::auto::compute_auto_frontlight_levels(
                            Local::now(),
                            coords,
                            night_brightness,
                            current_intensity,
                        );
                        if let Err(error) = context.frontlight.set_intensity(levels.intensity) {
                            tracing::error!(error = %error, "failed to set auto frontlight intensity");
                        }
                        if let Err(error) = context.frontlight.set_warmth(levels.warmth) {
                            tracing::error!(error = %error, "failed to set auto frontlight warmth");
                        }
                        context.settings.frontlight_levels = levels;
                    } else {
                        tracing::debug!("no coordinates available for auto-frontlight");
                    }
                }
            }
            Event::Open(info) => {
                open_document(
                    info,
                    &mut view,
                    &mut history,
                    &mut updating,
                    &raw_sender,
                    &tx,
                    &mut bus,
                    &mut rq,
                    &mut context,
                );
            }
            Event::Select(EntryId::About) => {
                let version_text = format!("{} {}", APP_NAME, get_version());

                let dialog = Dialog::builder(ViewId::AboutDialog, version_text)
                    .add_button("OK", Event::Close(ViewId::AboutDialog))
                    .add_button("Docs", Event::Select(EntryId::OpenDocumentation))
                    .build(&mut context);
                rq.add(RenderData::new(
                    dialog.id(),
                    *dialog.rect(),
                    UpdateMode::Gui,
                ));
                view.children_mut().push(Box::new(dialog) as Box<dyn View>);
            }
            Event::Select(EntryId::SystemInfo) => {
                view.children_mut().retain(|child| !child.is::<Menu>());
                let html = sys_info_as_html();
                let r = Reader::from_html(context.fb.rect(), &html, None, &tx, &mut context);
                let mut next_view = Box::new(r) as Box<dyn View>;
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome: context.fb.monochrome(),
                    dithered: context.fb.dithered(),
                });
                view = next_view;
            }
            Event::Select(EntryId::OpenDocumentation) => {
                view.children_mut().retain(|child| !child.is::<Menu>());

                if let Some(r) = open_documentation(context.fb.rect(), &tx, &mut context) {
                    let mut next_view = Box::new(r) as Box<dyn View>;
                    transfer_notifications(
                        view.as_mut(),
                        next_view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    history.push(HistoryItem {
                        view,
                        rotation: context.display.rotation,
                        monochrome: context.fb.monochrome(),
                        dithered: context.fb.dithered(),
                    });
                    view = next_view;
                } else {
                    let notif = Notification::new(
                        None,
                        "Failed to open documentation".to_string(),
                        false,
                        &tx,
                        &mut rq,
                        &mut context,
                    );
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                }
            }
            Event::OpenHtml(ref html, ref link_uri) => {
                view.children_mut().retain(|child| !child.is::<Menu>());
                let r = Reader::from_html(
                    context.fb.rect(),
                    html,
                    link_uri.as_deref(),
                    &tx,
                    &mut context,
                );
                let mut next_view = Box::new(r) as Box<dyn View>;
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome: context.fb.monochrome(),
                    dithered: context.fb.dithered(),
                });
                view = next_view;
            }
            Event::Select(EntryId::Launch(app_cmd)) => {
                view.children_mut().retain(|child| !child.is::<Menu>());
                let monochrome = context.fb.monochrome();
                let mut next_view: Box<dyn View> = match app_cmd {
                    AppCmd::Sketch => {
                        context.fb.set_monochrome(true);
                        Box::new(Sketch::new(context.fb.rect(), &mut rq, &mut context))
                    }
                    AppCmd::Calculator => Box::new(Calculator::new(
                        context.fb.rect(),
                        &tx,
                        &mut rq,
                        &mut context,
                    )?),
                    AppCmd::Dictionary {
                        ref query,
                        ref language,
                    } => Box::new(DictionaryApp::new(
                        context.fb.rect(),
                        query,
                        language,
                        &tx,
                        &mut rq,
                        &mut context,
                    )),
                    AppCmd::TouchEvents => {
                        Box::new(TouchEvents::new(context.fb.rect(), &mut rq, &mut context))
                    }
                    AppCmd::RotationValues => Box::new(RotationValues::new(
                        context.fb.rect(),
                        &mut rq,
                        &mut context,
                    )),
                    AppCmd::SettingsEditor => Box::new(SettingsEditor::new(
                        context.fb.rect(),
                        &mut rq,
                        &mut context,
                    )),
                };
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome,
                    dithered: context.fb.dithered(),
                });
                view = next_view;
            }
            Event::Back => {
                if let Some(mut item) = history.pop() {
                    transfer_notifications(
                        view.as_mut(),
                        item.view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    view = item.view;
                    if item.monochrome != context.fb.monochrome() {
                        context.fb.set_monochrome(item.monochrome);
                    }
                    if item.dithered != context.fb.dithered() {
                        context.fb.set_dithered(item.dithered);
                    }
                    if CURRENT_DEVICE.orientation(item.rotation)
                        != CURRENT_DEVICE.orientation(context.display.rotation)
                    {
                        wait_for_all(&mut updating, &mut context);
                        if let Ok(dims) = context.fb.set_rotation(item.rotation) {
                            raw_sender.send(display_rotate_event(item.rotation)).ok();
                            context.display.rotation = item.rotation;
                            context.display.dims = dims;
                        }
                    }
                    view.handle_event(&Event::Reseed, &tx, &mut bus, &mut rq, &mut context);
                } else if !view.is::<Home>() {
                    break;
                }
            }
            Event::TogglePresetMenu(rect, index) => {
                if let Some(index) = locate_by_id(view.as_ref(), ViewId::PresetMenu) {
                    let rect = *view.child(index).rect();
                    view.children_mut().remove(index);
                    rq.add(RenderData::expose(rect, UpdateMode::Gui));
                } else {
                    let preset_menu = Menu::new(
                        rect,
                        ViewId::PresetMenu,
                        MenuKind::Contextual,
                        vec![EntryKind::Command(
                            "Remove".to_string(),
                            EntryId::RemovePreset(index),
                        )],
                        &mut context,
                    );
                    rq.add(RenderData::new(
                        preset_menu.id(),
                        *preset_menu.rect(),
                        UpdateMode::Gui,
                    ));
                    view.children_mut()
                        .push(Box::new(preset_menu) as Box<dyn View>);
                }
            }
            Event::Show(ViewId::Frontlight) => {
                if !context.settings.frontlight {
                    context.set_frontlight(true);
                    view.handle_event(
                        &Event::ToggleFrontlight,
                        &tx,
                        &mut bus,
                        &mut rq,
                        &mut context,
                    );
                }
                let flw = FrontlightWindow::new(&mut context);
                rq.add(RenderData::new(flw.id(), *flw.rect(), UpdateMode::Gui));
                view.children_mut().push(Box::new(flw) as Box<dyn View>);
            }
            Event::ToggleInputHistoryMenu(id, rect) => {
                toggle_input_history_menu(view.as_mut(), id, rect, None, &mut rq, &mut context);
            }
            Event::ToggleNear(ViewId::KeyboardLayoutMenu, rect) => {
                toggle_keyboard_layout_menu(view.as_mut(), rect, None, &mut rq, &mut context);
            }
            Event::Close(ViewId::Frontlight) => {
                if let Some(index) = locate::<FrontlightWindow>(view.as_ref()) {
                    let rect = *view.child(index).rect();
                    view.children_mut().remove(index);
                    rq.add(RenderData::expose(rect, UpdateMode::Gui));
                }
            }
            Event::Close(id) => {
                if let Some(index) = locate_by_id(view.as_ref(), id) {
                    let rect = overlapping_rectangle(view.child(index));
                    rq.add(RenderData::expose(rect, UpdateMode::Gui));
                    view.children_mut().remove(index);
                }
            }
            Event::Select(EntryId::ToggleInverted) => {
                context.fb.toggle_inverted();
                context.settings.inverted = context.fb.inverted();
                rq.add(RenderData::new(
                    view.id(),
                    context.fb.rect(),
                    UpdateMode::Full,
                ));
            }
            Event::Select(EntryId::ToggleDithered) => {
                context.fb.toggle_dithered();
                rq.add(RenderData::new(
                    view.id(),
                    context.fb.rect(),
                    UpdateMode::Full,
                ));
            }
            Event::Select(EntryId::Rotate(n))
                if n != context.display.rotation && view.might_rotate() =>
            {
                wait_for_all(&mut updating, &mut context);
                if let Ok(dims) = context.fb.set_rotation(n) {
                    raw_sender.send(display_rotate_event(n)).ok();
                    context.display.rotation = n;
                    let fb_rect = Rectangle::from(dims);
                    if context.display.dims != dims {
                        context.display.dims = dims;
                        view.resize(fb_rect, &tx, &mut rq, &mut context);
                    } else {
                        rq.add(RenderData::new(
                            view.id(),
                            context.fb.rect(),
                            UpdateMode::Full,
                        ));
                    }
                }
            }
            Event::Select(EntryId::SetRotationLock(rotation_lock)) => {
                context.settings.rotation_lock = rotation_lock;
            }
            Event::Select(EntryId::SetButtonScheme(button_scheme)) => {
                context.settings.button_scheme = button_scheme;

                // Sending a pseudo event into the raw_events channel toggles the inversion in the device_events channel
                match button_scheme {
                    ButtonScheme::Natural => {
                        raw_sender.send(button_scheme_event(VAL_RELEASE)).ok();
                    }
                    ButtonScheme::Inverted => {
                        raw_sender.send(button_scheme_event(VAL_PRESS)).ok();
                    }
                }

                // Re-dispatch event to view hierarchy so UI can update
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
            }
            Event::SetWifi(enable) => {
                set_wifi(enable, &mut context);
            }
            Event::ReloadDictionaries => {
                context.load_dictionaries();
            }
            Event::Select(EntryId::CheckForUpdates) => {
                show_ota_view(view.as_mut(), &tx, &mut rq, &mut context);
            }
            Event::Select(EntryId::ToggleWifi) => {
                set_wifi(!context.settings.wifi, &mut context);
            }
            Event::Select(EntryId::TakeScreenshot) => {
                let name = Local::now().format("screenshot-%Y%m%d_%H%M%S.png");
                let msg = match context.fb.save(&name.to_string()) {
                    Err(e) => format!("{}", e),
                    Ok(_) => format!("Saved {}.", name),
                };
                let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                view.children_mut().push(Box::new(notif) as Box<dyn View>);
            }
            Event::CheckFetcher(..)
            | Event::FetcherAddDocument(..)
            | Event::FetcherRemoveDocument(..)
            | Event::FetcherSearch { .. }
                if !view.is::<Home>() =>
            {
                if let Some(entry) = history.get_mut(0).filter(|entry| entry.view.is::<Home>()) {
                    let (tx, _rx) = mpsc::channel();
                    entry.view.handle_event(
                        &evt,
                        &tx,
                        &mut VecDeque::new(),
                        &mut RenderQueue::new(),
                        &mut context,
                    );
                }
            }
            Event::Notification(notif_event) => match notif_event {
                NotificationEvent::Show(msg) => {
                    let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                }
                NotificationEvent::ShowPinned(id, msg) => {
                    let notif = Notification::new(Some(id), msg, true, &tx, &mut rq, &mut context);
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                }
                NotificationEvent::UpdateText(id, text) => {
                    if let Some(notif) = find_notification_mut(view.as_mut(), id) {
                        notif.update_text(text, &mut rq);
                    } else {
                        view.children_mut().push(Box::new(Notification::new(
                            Some(id),
                            text,
                            true,
                            &tx,
                            &mut rq,
                            &mut context,
                        )) as Box<dyn View>);
                    }
                }
                NotificationEvent::UpdateProgress(id, progress) => {
                    if let Some(notif) = find_notification_mut(view.as_mut(), id) {
                        notif.update_progress(progress, &mut rq);
                    }
                }
            },
            Event::Select(EntryId::Restart) => {
                exit_status = ExitStatus::Restart;
                break;
            }
            Event::Select(EntryId::Reboot) => {
                exit_status = ExitStatus::Reboot;
                break;
            }
            Event::Select(EntryId::Quit) => {
                break;
            }
            Event::Select(EntryId::PowerOff) => {
                power_off(view.as_mut(), &mut history, &mut updating, &mut context);
                exit_status = ExitStatus::PowerOff;
                break;
            }
            Event::Select(EntryId::Suspend) => {
                initiate_suspend(
                    view.as_mut(),
                    &tx,
                    &mut bus,
                    &mut rq,
                    &mut tasks,
                    &mut context,
                );
            }
            Event::MightSuspend if context.settings.auto_suspend > 0.0 => {
                if context.shared
                    || tasks
                        .iter()
                        .any(|task| task.id == TaskId::PrepareSuspend || task.id == TaskId::Suspend)
                {
                    inactive_since = Instant::now();
                    continue;
                }
                let seconds = 60.0 * context.settings.auto_suspend;
                if inactive_since.elapsed() > Duration::from_secs_f32(seconds) {
                    initiate_suspend(
                        view.as_mut(),
                        &tx,
                        &mut bus,
                        &mut rq,
                        &mut tasks,
                        &mut context,
                    );
                }
            }
            _ => {
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
            }
        }

        process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);

        while let Some(ce) = bus.pop_front() {
            tx.send(ce).ok();
        }
    }

    if exit_status == ExitStatus::Quit
        && !CURRENT_DEVICE.has_gyroscope()
        && context.display.rotation != initial_rotation
    {
        context.fb.set_rotation(initial_rotation).ok();
    }

    background_tasks.stop_all();

    if tasks.iter().all(|task| task.id != TaskId::Suspend) && context.settings.frontlight {
        context.settings.frontlight_levels = context.frontlight.levels();
    }

    let save_settings = match exit_status {
        ExitStatus::Restart | ExitStatus::Reboot => !context.shared,
        _ => true,
    };

    if save_settings {
        if let Err(e) = manager.save(&context.settings) {
            tracing::error!(error = ?e, "failed to save settings");
        }
    }

    match exit_status {
        ExitStatus::Restart => {
            File::create("/tmp/restart").ok();
        }
        ExitStatus::Reboot => {
            File::create("/tmp/reboot").ok();
        }
        ExitStatus::PowerOff => {
            File::create("/tmp/power_off").ok();
        }
        _ => {
            // nickel.sh killed wifi before starting nickel, so we're doing the same here.
            if let Ok(wifi) = CURRENT_DEVICE.wifi_manager() {
                if let Err(e) = wifi.disable() {
                    tracing::error!(error = %e, "Failed to disable WiFi on exit");
                }
            }
        }
    }

    match CURRENT_DEVICE.power_manager() {
        Ok(power) => {
            if let Err(e) = power.restore_cores() {
                tracing::error!(error = %e, "Failed to restore CPU cores on exit");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to retrieve power manager on exit");
        }
    }

    #[cfg(feature = "profiling")]
    cadmus_core::telemetry::profiling::shutdown_profiling();

    cadmus_core::logging::shutdown_logging();

    Ok(())
}
