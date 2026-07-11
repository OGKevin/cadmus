use cadmus_core::anyhow::{Context as ResultExt, Error, format_err};
use cadmus_core::assets::open_documentation;
use cadmus_core::chrono::Local;
use cadmus_core::db::Database;
use cadmus_core::device::AppContext;
use cadmus_core::device::AppDevice;
use cadmus_core::device::DeviceHardware as _;
use cadmus_core::device::DeviceRotation as _;
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
use std::sync::mpsc;
use std::time::Instant;
use tracing::{error, info, warn};

pub const APP_NAME: &str = "Cadmus";

fn drain_bus(bus: &mut Bus, tx: &Hub) {
    while let Some(ce) = bus.pop_front() {
        tx.send(ce).ok();
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
fn open_document(
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
                .parse()
                .ok()
                .is_some_and(|kind| context.settings.reader.dithered_kinds.contains(&kind)),
        );
    }

    let path = info.file.path.clone();
    if let Some(r) = Reader::new(context.device.framebuffer().rect(), *info, tx, context) {
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
        handle_event(view.as_mut(), &Event::Invalid(path), tx, bus, rq, context);
        false
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(device, settings, fonts, database), level = tracing::Level::TRACE))]
fn build_context(
    device: AppDevice,
    settings: Settings,
    fonts: Fonts,
    database: Database,
) -> Result<AppContext, Error> {
    let mut settings = settings;

    if settings.libraries.is_empty() {
        return Err(format_err!("no libraries found"));
    }

    if settings.selected_library >= settings.libraries.len() {
        settings.selected_library = 0;
    }

    let library_settings = &settings.libraries[settings.selected_library];
    let library = Library::new(&library_settings.path, &database, &library_settings.name)?;

    Ok(AppContext::new(device, library, database, settings, fonts))
}

pub fn run() -> Result<(), Error> {
    let start_time = Instant::now();

    let mut inactive_since = Instant::now();
    let mut exit_status = ExitStatus::Quit;

    let mut device = AppDevice::default();

    let manager = SettingsManager::new(device.data_dir(), get_current_version());
    let mut settings = manager.load();

    cadmus_core::crypto::init_crypto_provider();

    if let Err(e) = cadmus_core::logging::init_logging(
        &settings.logging,
        device.data_path(&settings.logging.directory),
    ) {
        eprintln!("Warning: Failed to initialize logging: {:#}", e);
        eprintln!("Continuing without logging...");
    }

    #[cfg(feature = "tracing")]
    let start_span =
        tracing::info_span!("app-start", version = ?get_version(), start_time = ?start_time)
            .entered();

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

    {
        #[cfg(feature = "tracing")]
        let _span = tracing::trace_span!(parent: &start_span, "startup-screen").entered();

        StartupScreen::show(&mut device, &mut fonts).ok();
    }

    let mut database = Database::new(device.resolve_db_path())
        .map_err(|e| {
            error!(error = %e, "can't open database");
            e
        })
        .context("can't open database")?;

    if let Err(e) = database.init(&device, settings.db_backup_retention, &mut settings) {
        error!(error = %e, "migrations failed");
        return Err(e);
    }

    if let Err(e) = manager.save(&settings) {
        error!(error = %e, "failed to save settings after migrations");
        return Err(e);
    }

    let database = database;

    let mut context =
        build_context(device, settings, fonts, database).context("can't build context")?;

    context.load_dictionaries();
    context.load_keyboard_layouts();

    let (tx, rx) = context
        .device
        .input_mut()
        .start(context.display, context.settings.button_scheme);

    let mut tasks: Vec<DeviceTask> = Vec::new();
    let mut background_tasks = TaskManager::new();

    cadmus_core::task::register_startup_tasks(
        &mut background_tasks,
        tx.clone(),
        &context.settings,
        &context.database,
        context.device.data_dir(),
        &context.device.install_dir(),
    );

    let mut history: Vec<HistoryItem> = Vec::new();
    let mut rq = RenderQueue::new();
    let mut view: Box<dyn View> = Box::new(Home::new(
        context.device.framebuffer().rect(),
        &mut rq,
        &mut context,
    )?);

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
        && let Some(info) = context.library.most_recently_opened_reading_book()
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
        );
    }

    AppDevice::on_startup(
        &mut context,
        &tx,
        &mut DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            inactive_since: &mut inactive_since,
            settings_manager: Some(&manager),
            startup_cwd: Some(&startup_cwd),
            background_tasks: Some(&mut background_tasks),
        },
    )?;

    #[cfg(feature = "tracing")]
    drop(start_span);

    tracing::info!(duration = ?start_time.elapsed(), "App started");

    while let Ok(evt) = rx.recv() {
        #[cfg(feature = "tracing")]
        let span = tracing::trace_span!("main-event-loop", event = ?evt);
        #[cfg(feature = "tracing")]
        let _enter = span.enter();
        #[cfg(feature = "tracing")]
        tracing::trace!(event = ?evt, "handling event");

        background_tasks.handle_event(&evt, &tx, &context);

        let mut runtime = DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            inactive_since: &mut inactive_since,
            settings_manager: Some(&manager),
            startup_cwd: Some(&startup_cwd),
            background_tasks: Some(&mut background_tasks),
        };

        match AppDevice::handle_event(&evt, &tx, &mut bus, &mut rq, &mut context, &mut runtime) {
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
                            tx.send(Event::Select(EntryId::TakeScreenshot)).ok();
                        }
                        _ => (),
                    }
                }
                _ => {
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
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
                let html = sys_info_as_html(context.device.model(), context.device.mark());
                let r = Reader::from_html(
                    context.device.framebuffer().rect(),
                    &html,
                    None,
                    &tx,
                    &mut context,
                );
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
                    open_documentation(context.device.framebuffer().rect(), &tx, &mut context)
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
                );
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
                    AppCmd::Calculator => Box::new(Calculator::new(
                        context.device.framebuffer().rect(),
                        &tx,
                        &mut rq,
                        &mut context,
                    )?),
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
                    AppCmd::SettingsEditor => Box::new(SettingsEditor::new(
                        context.device.framebuffer().rect(),
                        &mut rq,
                        &mut context,
                    )),
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
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
            }
            Event::ReloadDictionaries => {
                context.load_dictionaries();
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
            _ => {
                handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
            }
        }

        process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);
        drain_bus(&mut bus, &tx);
    }

    background_tasks.stop_all();

    if let Err(e) = AppDevice::on_shutdown(
        &mut context,
        exit_status,
        &mut DeviceRuntime {
            view: &mut view,
            history: &mut history,
            tasks: &mut tasks,
            updating: &mut updating,
            inactive_since: &mut inactive_since,
            settings_manager: Some(&manager),
            startup_cwd: Some(&startup_cwd),
            background_tasks: Some(&mut background_tasks),
        },
    ) {
        tracing::error!(error = %e, "Failed to run on_shutdown");
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

    #[cfg(feature = "profiling")]
    cadmus_core::telemetry::profiling::shutdown_profiling();

    cadmus_core::logging::shutdown_logging();

    Ok(())
}
