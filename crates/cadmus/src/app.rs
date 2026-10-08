use cadmus_core::anyhow::{Context as ResultExt, Error, format_err};
use cadmus_core::assets::open_documentation;
use cadmus_core::chrono::Local;
use cadmus_core::db::Database;
use cadmus_core::device::AppContext;
use cadmus_core::device::AppDevice;
use cadmus_core::device::DeviceHardware as _;
use cadmus_core::device::DeviceRotation as _;
use cadmus_core::device::inhibitor::{Kind, SoftSuspendName};
use cadmus_core::device::leds::{LedPattern, LedPriority, StatusLed};
use cadmus_core::device::rtc::shutdown_rtc;
use cadmus_core::device::wifi::WifiManager;
use cadmus_core::device::{
    DeviceIdentity, DeviceInput, DeviceLifecycle, DevicePaths, DeviceRuntime, DeviceTask,
    ExitStatus, HistoryItem, InputSource,
};
use cadmus_core::document::sys_info_as_html;
use cadmus_core::font::Fonts;
use cadmus_core::framebuffer::Framebuffer as _;
use cadmus_core::framebuffer::UpdateMode;
use cadmus_core::geom::{DiagDir, Rectangle, Region};
use cadmus_core::gesture::GestureEvent;
use cadmus_core::i18n;
use cadmus_core::input::DeviceEvent;
use cadmus_core::input::{VAL_PRESS, VAL_RELEASE};
use cadmus_core::input::{button_scheme_event, display_rotate_event};
use cadmus_core::library::Library;
use cadmus_core::metadata::Info;
use cadmus_core::settings::versioned::SettingsManager;
use cadmus_core::settings::{ButtonScheme, Settings, StartupMode};
use cadmus_core::task::TaskManager;
use cadmus_core::version::{get_current_version, get_version};
use cadmus_core::view::HubReceiver;
use cadmus_core::view::calculator::Calculator;
use cadmus_core::view::common::{
    find_notification_mut, locate, locate_by_id, overlapping_rectangle, transfer_notifications,
};
use cadmus_core::view::common::{toggle_input_history_menu, toggle_keyboard_layout_menu};
use cadmus_core::view::dialog::Dialog;
use cadmus_core::view::dictionary::Dictionary as DictionaryApp;
use cadmus_core::view::frontlight::FrontlightWindow;
use cadmus_core::view::home::Home;
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
use std::path::PathBuf;
use std::time::Instant;
use tracing::{error, info, warn};

pub const APP_NAME: &str = "Cadmus";

/// The main loop's mutable state, borrowed by shutdown.
struct ShutdownState<'a> {
    view: &'a mut Box<dyn View>,
    history: &'a mut Vec<HistoryItem>,
    tasks: &'a mut Vec<DeviceTask>,
    updating: &'a mut Vec<UpdateData>,
    bus: &'a mut Bus,
    rq: &'a mut RenderQueue,
    context: &'a mut AppContext,
    settings_manager: &'a SettingsManager,
    startup_cwd: &'a Option<PathBuf>,
    background_tasks: &'a mut TaskManager,
}

/// Stops every remaining event producer, then drains the shutdown-related
/// events they already queued.
///
/// Draining comes after the producers stop so no new event arrives mid-drain,
/// and the main loop has usually already broken by the time a committed OTA
/// install queues its reboot.
///
/// View-owned [`Job`](cadmus_core::runtime::Job)s are cancelled while each view
/// still holds its handle, then moved out and joined. Reversing that order
/// clears the handle before cancellation runs, so the job never receives stop.
///
/// Only shutdown-related events are acted on: [`Event::CheckBattery`], which can
/// return a low-battery `PowerOff` and records battery state before teardown, and
/// the direct exit intents (`Select(PowerOff|Restart|Reboot|Quit)` and
/// [`Event::Quit`]). The exit intents are dispatched through the normal device
/// handler so `user_exit_blocked`, the power-off intermission, and the emulator's
/// `Reboot` → `Quit` mapping still apply. Everything else is dropped — replaying an
/// arbitrary queued device event here would run teardown-hostile transitions such
/// as `PrepareShare` (which closes the database and enables mass storage) after
/// the process has begun exiting. An exit is merged with `exit_status` using fixed
/// precedence (`Reboot` > `Restart` > `PowerOff` > `Quit`).
async fn stop_producers_and_drain(
    tx: &Hub,
    rx: &mut HubReceiver,
    state: &mut ShutdownState<'_>,
    exit_status: ExitStatus,
) -> ExitStatus {
    let ShutdownState {
        view,
        history,
        tasks,
        updating,
        bus,
        rq,
        context,
        settings_manager,
        startup_cwd,
        background_tasks,
    } = state;

    tasks.clear();
    for item in history.iter() {
        cadmus_core::view::cancel_view_jobs(item.view.as_ref());
    }
    cadmus_core::view::cancel_view_jobs(view.as_ref());

    let mut view_jobs = Vec::new();
    for item in history.iter_mut() {
        cadmus_core::view::take_view_jobs(item.view.as_mut(), &mut view_jobs);
    }
    cadmus_core::view::take_view_jobs(view.as_mut(), &mut view_jobs);
    let _ = cadmus_core::runtime::finish_within_deadline(
        view_jobs,
        cadmus_core::runtime::SHUTDOWN_DEADLINE,
    )
    .await;

    let mut drained_exit = exit_status;
    let mut runtime = cadmus_core::device::DeviceRuntime {
        view,
        history,
        tasks,
        updating,
        settings_manager: Some(settings_manager),
        startup_cwd: Some(startup_cwd),
        background_tasks: Some(background_tasks),
    };

    while let Ok(message) = rx.try_recv() {
        let (evt, _input_wake) = message.into_parts();
        if !is_shutdown_related_event(&evt) {
            continue;
        }
        let outcome = AppDevice::handle_event(&evt, tx, bus, rq, context, &mut runtime).await;
        if let cadmus_core::device::EventOutcome::Exit(status) = outcome {
            drained_exit = merge_shutdown_exit_status(drained_exit, status);
        }
    }
    drained_exit
}

/// Whether a queued device event must still run during shutdown.
///
/// [`Event::CheckBattery`] can return a low-battery [`ExitStatus::PowerOff`] and
/// records the battery level before the device goes down. The direct exit intents
/// are replayed through the device handler (not short-circuited) so that
/// `user_exit_blocked`, the power-off intermission, and the emulator's `Reboot` →
/// `Quit` mapping still run. Any other event can only mutate UI or platform state
/// that teardown is already dismantling, so it is dropped rather than replayed.
fn is_shutdown_related_event(evt: &Event) -> bool {
    matches!(
        evt,
        Event::CheckBattery
            | Event::Quit
            | Event::Select(EntryId::PowerOff)
            | Event::Select(EntryId::Restart)
            | Event::Select(EntryId::Reboot)
            | Event::Select(EntryId::Quit)
    )
}

/// Exit intents queued during shutdown. Higher precedence wins when several are
/// drained together (for example OTA `Reboot` must not be downgraded by `Quit`).
fn merge_shutdown_exit_status(current: ExitStatus, candidate: ExitStatus) -> ExitStatus {
    if shutdown_exit_precedence(&candidate) > shutdown_exit_precedence(&current) {
        candidate
    } else {
        current
    }
}

fn shutdown_exit_precedence(status: &ExitStatus) -> u8 {
    match status {
        ExitStatus::Reboot => 5,
        ExitStatus::Restart => 4,
        ExitStatus::PowerOff => 3,
        ExitStatus::RunCommand(_) => 3,
        ExitStatus::Quit => 1,
    }
}

fn drain_bus(bus: &mut Bus, tx: &Hub) {
    while let Some(ce) = bus.pop_front() {
        tx.send(ce.into()).ok();
    }
}

struct MainLoopSoftSuspendGuard {
    _lease: cadmus_core::device::inhibitor::InhibitorGuard,
}

impl Drop for MainLoopSoftSuspendGuard {
    fn drop(&mut self) {
        tracing::trace!(
            soft_suspend_lease = "main-loop",
            "soft-suspend lease released for main-loop event"
        );
    }
}

/// Keeps the root view in sync after a rotation changes framebuffer dimensions.
fn sync_view_after_rotation(
    view: &mut Box<dyn View>,
    prev_dims: (u32, u32),
    dims: (u32, u32),
    tx: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    let fb_rect = Rectangle::from(dims);
    if prev_dims != dims {
        view.resize(fb_rect, tx, rq, context);
    } else {
        rq.add(RenderData::new(
            view.id(),
            context.device.framebuffer().rect(),
            UpdateMode::Full,
        ));
    }
}

/// Applies a rotation and keeps the root view in sync.
///
/// Waits for pending framebuffer updates, then calls
/// [`AppContext::set_rotation`]. On success, injects a synthetic
/// `KEY_ROTATE_DISPLAY` event so the input subsystem remaps touch coordinates.
///
/// Returns `true` if the rotation was applied successfully.
fn apply_rotation(
    rotation: i8,
    view: &mut Box<dyn View>,
    updating: &mut Vec<UpdateData>,
    tx: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) -> bool {
    let prev_dims = context.display.dims;
    wait_for_all(updating, context);
    if let Ok(dims) = context.set_rotation(rotation) {
        context
            .device
            .input()
            .send_raw(display_rotate_event(rotation));
        sync_view_after_rotation(view, prev_dims, dims, tx, rq, context);
        true
    } else {
        false
    }
}

/// Rotates the display before opening a document, without resizing the outgoing view.
///
/// The current view is replaced immediately afterward by [`Reader::new`], so syncing
/// its layout would be redundant work.
fn set_rotation(rotation: i8, updating: &mut Vec<UpdateData>, context: &mut AppContext) -> bool {
    wait_for_all(updating, context);
    if context.set_rotation(rotation).is_ok() {
        context
            .device
            .input()
            .send_raw(display_rotate_event(rotation));
        true
    } else {
        false
    }
}

#[allow(clippy::too_many_arguments)]
// TODO(OGKevin): This shall be moved to the readerm module
#[cfg_attr(feature = "tracing", tracing::instrument(skip(info, view, history, updating, tx, bus, rq, context), level = tracing::Level::TRACE))]
async fn open_document(
    info: Box<Info>,
    view: &mut Box<dyn View>,
    history: &mut Vec<HistoryItem>,
    updating: &mut Vec<UpdateData>,
    tx: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) -> bool {
    let rotation = context.display.rotation;
    let dithered = context.device.framebuffer().dithered();

    if let Some(reader_info) = info.reader.as_ref() {
        if let Some(n) = reader_info.rotation.map(|n| context.device.to_native(n))
            && context.device.orientation(n) != context.device.orientation(rotation)
        {
            set_rotation(n, updating, context);
        }
        context
            .device
            .framebuffer_mut()
            .set_dithered(reader_info.dithered);
    } else {
        context.device.framebuffer_mut().set_dithered(
            info.file
                .kind
                .is_some_and(|kind| context.settings.reader.dithered_kinds.contains(&kind)),
        );
    }

    let path = info.file.path.clone();
    if let Some(r) = Reader::new(context.device.framebuffer().rect(), *info, tx, context).await {
        let mut next_view = Box::new(r) as Box<dyn View>;
        transfer_notifications(view.as_mut(), next_view.as_mut(), rq, context);
        if view.is::<Reader>() {
            *view = next_view;
        } else {
            let prev = std::mem::replace(view, next_view);
            history.push(HistoryItem {
                view: prev,
                rotation,
                monochrome: context.device.framebuffer().monochrome(),
                dithered,
            });
        }
        true
    } else {
        if context.display.rotation != rotation {
            apply_rotation(rotation, view, updating, tx, rq, context);
        }
        context.device.framebuffer_mut().set_dithered(dithered);
        warn!(
            path = %path.display(),
            library_home = %context.library.home.display(),
            "Reader::new returned None, dispatching Event::Invalid"
        );
        handle_event(view.as_mut(), &Event::Invalid(path), tx, bus, rq, context).await;
        false
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(device, settings, fonts, database, status_led), level = tracing::Level::TRACE))]
async fn build_context(
    device: AppDevice,
    settings: Settings,
    fonts: Fonts,
    database: Database,
    status_led: std::sync::Arc<StatusLed>,
) -> Result<AppContext, Error> {
    let mut settings = settings;

    if settings.libraries.is_empty() {
        return Err(format_err!("no libraries found"));
    }

    if settings.selected_library >= settings.libraries.len() {
        settings.selected_library = 0;
    }

    let library_settings = &settings.libraries[settings.selected_library];
    let library = Library::new(&library_settings.path, &database, &library_settings.name).await?;

    Ok(AppContext::new(device, library, database, settings, fonts, status_led).await)
}

/// Application entry after the process runtime is running.
///
/// When the `tracing` feature is enabled, startup uses a parent `info_span!`
/// passed to child work via [`.instrument(...)`](tracing::Instrument), not
/// [`Span::enter()`](tracing::Span::enter), because entered guards are not
/// `Send` and would leak the span across awaits on a Tokio worker.
pub async fn run() -> Result<(), Error> {
    let start_time = Instant::now();

    let mut exit_status = ExitStatus::Quit;

    let mut device = AppDevice::from_environment()
        .await
        .expect("failed to initialize device");

    let manager = SettingsManager::new(device.data_dir(), get_current_version());
    let mut settings = manager.load();

    cadmus_core::crypto::init_crypto_provider();

    if let Err(e) = cadmus_core::logging::init_logging(
        &settings.logging,
        device.data_path(&settings.logging.directory),
    )
    .await
    {
        eprintln!("Warning: Failed to initialize logging: {:#}", e);
        eprintln!("Continuing without logging...");
    }

    #[cfg(feature = "tracing")]
    let start_span =
        tracing::info_span!("app-start", version = ?get_version(), start_time = ?start_time);

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
    device.clean_tmp_dir();

    let mut fonts = Fonts::load(&device.install_dir()).context("can't load fonts")?;

    let status_led = StatusLed::new(device.device_leds());
    let startup_led = status_led.install(
        "startup",
        LedPriority::Startup,
        LedPattern::Blink {
            on: std::time::Duration::from_millis(500),
            off: std::time::Duration::from_millis(500),
        },
    );
    {
        #[cfg(feature = "tracing")]
        let _span = tracing::trace_span!(parent: &start_span, "startup-screen").entered();

        StartupScreen::show(&mut device, &mut fonts).ok();
    };

    let mut database = Database::new(device.resolve_db_path())
        .await
        .map_err(|e| {
            error!(error = %e, "can't open database");
            e
        })
        .context("can't open database")?;

    if let Err(e) = database
        .init(&device, settings.db_backup_retention, &mut settings)
        .await
    {
        error!(error = %e, "migrations failed");
        return Err(e);
    }

    if let Err(e) = manager.save(&settings) {
        error!(error = %e, "failed to save settings after migrations");
        return Err(e);
    }

    let database = database;

    let mut context = build_context(
        device,
        settings,
        fonts,
        database,
        std::sync::Arc::clone(&status_led),
    )
    .await
    .context("can't build context")?;

    context.load_dictionaries().await;
    context.load_keyboard_layouts().await;

    let (tx, mut rx) = context.device.input_mut().start(
        context.display,
        context.settings.button_scheme,
        std::sync::Arc::clone(&context.inhibitor),
    );

    let mut tasks: Vec<DeviceTask> = Vec::new();
    let mut background_tasks = TaskManager::new();

    cadmus_core::task::register_startup_tasks(
        &mut background_tasks,
        tx.clone(),
        &context.settings,
        &context.database,
        context.device.data_dir(),
        &context.device.install_dir(),
        &context.inhibitor,
    );

    let mut history: Vec<HistoryItem> = Vec::new();
    let mut rq = RenderQueue::new();
    let mut view: Box<dyn View> =
        Box::new(Home::new(context.device.framebuffer().rect(), &mut rq, &mut context).await?);

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
        context.device.model()
    );
    info!(
        "The framebuffer resolution is {} by {}.",
        context.device.framebuffer().rect().width(),
        context.device.framebuffer().rect().height()
    );

    let mut bus = VecDeque::with_capacity(4);

    if context.settings.startup_mode == StartupMode::LastFile
        && let Some(info) = context.library.most_recently_opened_reading_book().await
    {
        open_document(
            Box::new(info),
            &mut view,
            &mut history,
            &mut updating,
            &tx,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
    }

    context.wifi_session.set_hub(tx.clone());

    AppDevice::on_startup(
        &mut context,
        &tx,
        &mut DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            settings_manager: Some(&manager),
            startup_cwd: Some(&startup_cwd),
            background_tasks: Some(&mut background_tasks),
        },
    )
    .await?;

    tracing::info!(duration = ?start_time.elapsed(), "App started");

    context.release_startup_lease();
    drop(startup_led);

    while let Some(message) = rx.recv().await {
        let (evt, _input_wake) = message.into_parts();
        let skip_main_loop_lease =
            AppDevice::should_skip_main_loop_soft_suspend_lease(&context, &evt);
        let _soft_suspend = if skip_main_loop_lease {
            tracing::trace!(
                event = ?evt,
                "skipping main-loop soft-suspend lease during deep-idle cycle"
            );
            None
        } else {
            match context
                .inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::MainLoop)
            {
                Ok(_lease) => {
                    tracing::trace!(
                        soft_suspend_lease = "main-loop",
                        event = ?evt,
                        "soft-suspend lease acquired for main-loop event"
                    );
                    Some(MainLoopSoftSuspendGuard { _lease })
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        soft_suspend_lease = "main-loop",
                        event = ?evt,
                        "failed to acquire soft-suspend lease for main-loop event"
                    );
                    None
                }
            }
        };

        #[cfg(feature = "tracing")]
        let span = tracing::trace_span!("main-event-loop", event = ?evt);

        #[cfg(feature = "tracing")]
        tracing::trace!(
            soft_suspend_lease = "main-loop",
            event = ?evt,
            "handling event"
        );
        #[cfg(feature = "tracing")]
        {
            use tracing::Instrument as _;
            background_tasks
                .handle_event(&evt, &tx, &context)
                .instrument(span.clone())
                .await;
        }
        #[cfg(not(feature = "tracing"))]
        {
            background_tasks.handle_event(&evt, &tx, &context).await;
        }

        let mut runtime = DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            settings_manager: Some(&manager),
            startup_cwd: Some(&startup_cwd),
            background_tasks: Some(&mut background_tasks),
        };

        #[cfg(feature = "tracing")]
        let device_outcome = {
            use tracing::Instrument as _;
            AppDevice::handle_event(&evt, &tx, &mut bus, &mut rq, &mut context, &mut runtime)
                .instrument(span.clone())
                .await
        };
        #[cfg(not(feature = "tracing"))]
        let device_outcome =
            AppDevice::handle_event(&evt, &tx, &mut bus, &mut rq, &mut context, &mut runtime).await;
        match device_outcome {
            cadmus_core::device::EventOutcome::Handled => {
                process_render_queue(view.as_mut(), &mut rq, &mut context, &mut updating);
                drain_bus(&mut bus, &tx);
                continue;
            }
            cadmus_core::device::EventOutcome::Error => {
                drain_bus(&mut bus, &tx);
                continue;
            }
            cadmus_core::device::EventOutcome::Exit(status) => {
                exit_status = status;
                break;
            }
            cadmus_core::device::EventOutcome::Continue
            | cadmus_core::device::EventOutcome::Unhandled => {}
        }

        // TODO(OGKevin): This shall be breaken down and moved into smaller functions.
        match evt {
            Event::Gesture(ge) => match ge {
                GestureEvent::MultiTap(mut points) => {
                    if points[0].x > points[1].x {
                        points.swap(0, 1);
                    }
                    let rect = context.device.framebuffer().rect();
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
                                context.device.framebuffer().rect(),
                                UpdateMode::Full,
                            ));
                        }
                        (
                            Region::Corner(DiagDir::NorthWest),
                            Region::Corner(DiagDir::SouthEast),
                        ) => {
                            tx.send(Event::Select(EntryId::TakeScreenshot).into()).ok();
                        }
                        _ => (),
                    }
                }
                _ => {
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context).await;
                }
            },
            Event::Open(info) => {
                open_document(
                    info,
                    &mut view,
                    &mut history,
                    &mut updating,
                    &tx,
                    &mut bus,
                    &mut rq,
                    &mut context,
                )
                .await;
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
                let network = match context.device.wifi_manager() {
                    Ok(wifi) => wifi
                        .network_info()
                        .await
                        .inspect_err(|e| {
                            tracing::warn!(error = %e, "no network info for system info");
                        })
                        .ok()
                        .flatten(),
                    Err(e) => {
                        tracing::warn!(error = %e, "no network info for system info");
                        None
                    }
                };
                let html = sys_info_as_html(
                    context.device.model(),
                    context.device.mark(),
                    network.as_ref(),
                );
                let r = Reader::from_html(
                    context.device.framebuffer().rect(),
                    &html,
                    None,
                    &tx,
                    &mut context,
                )
                .await;
                let mut next_view = Box::new(r) as Box<dyn View>;
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome: context.device.framebuffer().monochrome(),
                    dithered: context.device.framebuffer().dithered(),
                });
                view = next_view;
            }
            Event::Select(EntryId::OpenDocumentation) => {
                view.children_mut().retain(|child| !child.is::<Menu>());

                if let Some(r) =
                    open_documentation(context.device.framebuffer().rect(), &tx, &mut context).await
                {
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
                        monochrome: context.device.framebuffer().monochrome(),
                        dithered: context.device.framebuffer().dithered(),
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
                    context.device.framebuffer().rect(),
                    html,
                    link_uri.as_deref(),
                    &tx,
                    &mut context,
                )
                .await;
                let mut next_view = Box::new(r) as Box<dyn View>;
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome: context.device.framebuffer().monochrome(),
                    dithered: context.device.framebuffer().dithered(),
                });
                view = next_view;
            }
            Event::Select(EntryId::Launch(app_cmd)) => {
                view.children_mut().retain(|child| !child.is::<Menu>());
                let monochrome = context.device.framebuffer().monochrome();
                let mut next_view: Box<dyn View> = match app_cmd {
                    AppCmd::Sketch => {
                        context.device.framebuffer_mut().set_monochrome(true);
                        Box::new(Sketch::new(
                            context.device.framebuffer().rect(),
                            &mut rq,
                            &mut context,
                        ))
                    }
                    AppCmd::Calculator => Box::new(
                        Calculator::new(
                            context.device.framebuffer().rect(),
                            &tx,
                            &mut rq,
                            &mut context,
                        )
                        .await?,
                    ),
                    AppCmd::Dictionary {
                        ref query,
                        ref language,
                    } => Box::new(DictionaryApp::new(
                        context.device.framebuffer().rect(),
                        query,
                        language,
                        &tx,
                        &mut rq,
                        &mut context,
                    )),
                    AppCmd::TouchEvents => Box::new(TouchEvents::new(
                        context.device.framebuffer().rect(),
                        &mut rq,
                        &mut context,
                    )),
                    AppCmd::RotationValues => Box::new(RotationValues::new(
                        context.device.framebuffer().rect(),
                        &mut rq,
                        &mut context,
                    )),
                    AppCmd::SettingsEditor => Box::new(
                        SettingsEditor::new(
                            context.device.framebuffer().rect(),
                            &mut rq,
                            &mut context,
                        )
                        .await,
                    ),
                };
                transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
                history.push(HistoryItem {
                    view,
                    rotation: context.display.rotation,
                    monochrome,
                    dithered: context.device.framebuffer().dithered(),
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
                    if item.monochrome != context.device.framebuffer().monochrome() {
                        context
                            .device
                            .framebuffer_mut()
                            .set_monochrome(item.monochrome);
                    }
                    if item.dithered != context.device.framebuffer().dithered() {
                        context.device.framebuffer_mut().set_dithered(item.dithered);
                    }
                    if context.device.orientation(item.rotation)
                        != context.device.orientation(context.display.rotation)
                    {
                        apply_rotation(
                            item.rotation,
                            &mut view,
                            &mut updating,
                            &tx,
                            &mut rq,
                            &mut context,
                        );
                    }
                    view.handle_event(&Event::Reseed, &tx, &mut bus, &mut rq, &mut context)
                        .await;
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
                    )
                    .await;
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
                context.device.framebuffer_mut().toggle_inverted();
                context.settings.inverted = context.device.framebuffer().inverted();
                rq.add(RenderData::new(
                    view.id(),
                    context.device.framebuffer().rect(),
                    UpdateMode::Full,
                ));
            }
            Event::Select(EntryId::ToggleDithered) => {
                context.device.framebuffer_mut().toggle_dithered();
                rq.add(RenderData::new(
                    view.id(),
                    context.device.framebuffer().rect(),
                    UpdateMode::Full,
                ));
            }
            Event::Select(EntryId::Rotate(n))
                if n != context.display.rotation && view.might_rotate() =>
            {
                apply_rotation(n, &mut view, &mut updating, &tx, &mut rq, &mut context);
            }
            Event::Select(EntryId::SetRotationLock(rotation_lock)) => {
                context.settings.rotation_lock = rotation_lock;
            }
            Event::Select(EntryId::SetButtonScheme(button_scheme)) => {
                context.settings.button_scheme = button_scheme;

                // Sending a pseudo event into the raw_events channel toggles the inversion in the device_events channel
                match button_scheme {
                    ButtonScheme::Natural => {
                        context
                            .device
                            .input()
                            .send_raw(button_scheme_event(VAL_RELEASE));
                    }
                    ButtonScheme::Inverted => {
                        context
                            .device
                            .input()
                            .send_raw(button_scheme_event(VAL_PRESS));
                    }
                }

                // Re-dispatch event to view hierarchy so UI can update
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context).await;
            }
            Event::ReloadDictionaries => {
                context.load_dictionaries().await;
            }
            Event::Select(EntryId::CheckForUpdates) => {
                show_ota_view(view.as_mut(), &tx, &mut rq, &mut context);
            }
            Event::Select(EntryId::TakeScreenshot) => {
                let name = Local::now().format("screenshot-%Y%m%d_%H%M%S.png");
                let msg = match context.device.framebuffer().save(&name.to_string()) {
                    Err(e) => format!("{}", e),
                    Ok(_) => format!("Saved {}.", name),
                };
                let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                view.children_mut().push(Box::new(notif) as Box<dyn View>);
            }
            // NetUp is handled by device lifecycle first (sets context.online +
            // notification), then forwarded here via Continue when Home is not
            // active so fetchers can react.
            // TODO(OGKevin): this needs to be refactored so that eventually this inline
            // comment can also be removed.
            Event::Device(DeviceEvent::NetUp)
            | Event::CheckFetcher(..)
            | Event::FetcherAddDocument(..)
            | Event::FetcherRemoveDocument(..)
            | Event::FetcherSearch { .. }
                if !view.is::<Home>() =>
            {
                if let Some(entry) = history.get_mut(0).filter(|entry| entry.view.is::<Home>()) {
                    let (tx, _rx) = cadmus_core::view::hub_channel();
                    entry
                        .view
                        .handle_event(
                            &evt,
                            &tx,
                            &mut VecDeque::new(),
                            &mut RenderQueue::new(),
                            &mut context,
                        )
                        .await;
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
            _ => {
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context).await;
            }
        }

        process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);
        drain_bus(&mut bus, &tx);
    }

    let _shutdown_wake = match context
        .inhibitor
        .acquire(Kind::SoftSuspend, SoftSuspendName::Shutdown)
    {
        Ok(guard) => Some(guard),
        Err(error) => {
            tracing::error!(
                error = %error,
                soft_suspend_lease = "shutdown",
                "failed to acquire soft-suspend lease for shutdown"
            );
            None
        }
    };

    background_tasks.stop_all().await;

    exit_status = stop_producers_and_drain(
        &tx,
        &mut rx,
        &mut ShutdownState {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            bus: &mut bus,
            rq: &mut rq,
            context: &mut context,
            settings_manager: &manager,
            startup_cwd: &startup_cwd,
            background_tasks: &mut background_tasks,
        },
        exit_status,
    )
    .await;

    shutdown_rtc(&context).await;

    let save_settings = match &exit_status {
        ExitStatus::Restart | ExitStatus::Reboot => !context.shared,
        _ => true,
    };

    if let Err(e) = AppDevice::on_shutdown(&mut context.shutdown(), exit_status, &tasks).await {
        tracing::error!(error = %e, "Failed to run on_shutdown");
    }

    if save_settings {
        if let Err(e) = manager.save(&context.settings) {
            tracing::error!(error = ?e, "failed to save settings");
        }
    }

    #[cfg(feature = "profiling")]
    cadmus_core::telemetry::profiling::shutdown_profiling();

    status_led.shutdown().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reboot_outranks_a_later_quit() {
        let merged = merge_shutdown_exit_status(ExitStatus::Reboot, ExitStatus::Quit);
        assert!(matches!(merged, ExitStatus::Reboot));
    }

    #[test]
    fn a_queued_power_off_is_not_downgraded_by_quit() {
        let merged = merge_shutdown_exit_status(ExitStatus::PowerOff, ExitStatus::Quit);
        assert!(matches!(merged, ExitStatus::PowerOff));
    }

    #[test]
    fn restart_outranks_power_off() {
        let merged = merge_shutdown_exit_status(ExitStatus::PowerOff, ExitStatus::Restart);
        assert!(matches!(merged, ExitStatus::Restart));
    }

    #[test]
    fn exit_selects_are_replayed_through_the_handler() {
        assert!(is_shutdown_related_event(&Event::Select(EntryId::Reboot)));
        assert!(is_shutdown_related_event(&Event::Select(EntryId::Restart)));
        assert!(is_shutdown_related_event(&Event::Select(EntryId::PowerOff)));
        assert!(is_shutdown_related_event(&Event::Select(EntryId::Quit)));
        assert!(is_shutdown_related_event(&Event::Quit));
    }

    #[test]
    fn only_shutdown_related_events_are_replayed_through_the_handler() {
        assert!(is_shutdown_related_event(&Event::CheckBattery));
        assert!(!is_shutdown_related_event(&Event::PrepareShare));
        assert!(!is_shutdown_related_event(&Event::Share));
        assert!(!is_shutdown_related_event(&Event::ToggleFrontlight));
    }
}
