use cadmus_core::anyhow::{Context as ResultExt, Error};
use cadmus_core::assets::open_documentation;
use cadmus_core::battery::{Battery, FakeBattery};
use cadmus_core::chrono::Local;
use cadmus_core::color::Color;
use cadmus_core::context::Context;
use cadmus_core::device::CURRENT_DEVICE;
use cadmus_core::document::sys_info_as_html;
use cadmus_core::font::Fonts;
use cadmus_core::framebuffer::{Framebuffer, UpdateMode};
use cadmus_core::frontlight::{Frontlight, LightLevels};
use cadmus_core::geom::{Axis, Rectangle};
use cadmus_core::gesture::{gesture_events, GestureEvent};
use cadmus_core::helpers::{load_toml, save_toml};
use cadmus_core::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use cadmus_core::library::Library;
use cadmus_core::lightsensor::LightSensor;
use cadmus_core::png;
use cadmus_core::pt;
use cadmus_core::settings::{IntermKind, Settings, SETTINGS_PATH};
use cadmus_core::view::calculator::Calculator;
use cadmus_core::view::common::{
    find_notification_mut, locate, locate_by_id, overlapping_rectangle, transfer_notifications,
};
use cadmus_core::view::common::{toggle_input_history_menu, toggle_keyboard_layout_menu};
use cadmus_core::view::dialog::Dialog;
use cadmus_core::view::dictionary::Dictionary;
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
use cadmus_core::view::touch_events::TouchEvents;
use cadmus_core::view::{
    handle_event, process_render_queue, wait_for_all, RenderData, RenderQueue,
};
use cadmus_core::view::{AppCmd, EntryId, EntryKind, Event, NotificationEvent, View, ViewId};
use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::mouse::MouseState;
use sdl2::pixels::{Color as SdlColor, PixelFormatEnum};
use sdl2::rect::Point as SdlPoint;
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, WindowCanvas};
use std::collections::VecDeque;
use std::fs::File;
use std::mem;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing::info;

pub const APP_NAME: &str = "Cadmus";
const DEFAULT_ROTATION: i8 = 1;

const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub fn build_context(fb: Box<dyn Framebuffer>) -> Result<Context, Error> {
    let mut settings = load_toml::<Settings, _>(SETTINGS_PATH)?;
    settings.wifi = true;
    let settings = settings;

    // Initialize logging
    cadmus_core::logging::init_logging(&settings.logging)
        .context("Failed to initialize logging")?;

    info!("Starting Cadmus emulator");

    let library_settings = &settings.libraries[settings.selected_library];
    let library = Library::new(&library_settings.path, library_settings.mode)?;

    let battery = Box::new(FakeBattery::new()) as Box<dyn Battery>;
    let frontlight = Box::new(LightLevels::default()) as Box<dyn Frontlight>;
    let lightsensor = Box::new(0u16) as Box<dyn LightSensor>;
    let fonts = Fonts::load()?;

    Ok(Context::new(
        fb,
        None,
        library,
        settings,
        fonts,
        battery,
        frontlight,
        lightsensor,
    ))
}

#[inline]
fn seconds(timestamp: u32) -> f64 {
    timestamp as f64 / 1000.0
}

#[inline]
pub fn device_event(event: SdlEvent) -> Option<DeviceEvent> {
    match event {
        SdlEvent::MouseButtonDown {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Down,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        SdlEvent::MouseButtonUp {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Up,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        SdlEvent::MouseMotion {
            timestamp, x, y, ..
        } => Some(DeviceEvent::Finger {
            id: 0,
            status: FingerStatus::Motion,
            position: pt!(x, y),
            time: seconds(timestamp),
        }),
        _ => None,
    }
}

fn code_from_key(key: Scancode) -> Option<ButtonCode> {
    match key {
        Scancode::B => Some(ButtonCode::Backward),
        Scancode::F => Some(ButtonCode::Forward),
        Scancode::P => Some(ButtonCode::Power),
        Scancode::L => Some(ButtonCode::Light),
        Scancode::H => Some(ButtonCode::Home),
        Scancode::E => Some(ButtonCode::Erase),
        Scancode::G => Some(ButtonCode::Highlight),
        _ => None,
    }
}

struct FBCanvas(WindowCanvas);

impl Framebuffer for FBCanvas {
    fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        let [red, green, blue] = color.rgb();
        self.0.set_draw_color(SdlColor::RGB(red, green, blue));
        self.0
            .draw_point(SdlPoint::new(x as i32, y as i32))
            .unwrap();
    }

    fn set_blended_pixel(&mut self, x: u32, y: u32, color: Color, alpha: f32) {
        let [red, green, blue] = color.rgb();
        self.0
            .set_draw_color(SdlColor::RGBA(red, green, blue, (alpha * 255.0) as u8));
        self.0
            .draw_point(SdlPoint::new(x as i32, y as i32))
            .unwrap();
    }

    fn invert_region(&mut self, rect: &Rectangle) {
        let width = rect.width();
        let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
        if let Ok(data) = self.0.read_pixels(s_rect, PixelFormatEnum::RGB24) {
            for y in rect.min.y..rect.max.y {
                let v = (y - rect.min.y) as u32;
                for x in rect.min.x..rect.max.x {
                    let u = (x - rect.min.x) as u32;
                    let addr = 3 * (v * width + u);
                    let red = data[addr as usize];
                    let green = data[(addr + 1) as usize];
                    let blue = data[(addr + 2) as usize];
                    let mut color = Color::Rgb(red, green, blue);
                    color.invert();
                    self.set_pixel(x as u32, y as u32, color);
                }
            }
        }
    }

    fn shift_region(&mut self, rect: &Rectangle, drift: u8) {
        let width = rect.width();
        let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
        if let Ok(data) = self.0.read_pixels(s_rect, PixelFormatEnum::RGB24) {
            for y in rect.min.y..rect.max.y {
                let v = (y - rect.min.y) as u32;
                for x in rect.min.x..rect.max.x {
                    let u = (x - rect.min.x) as u32;
                    let addr = 3 * (v * width + u);
                    let red = data[addr as usize];
                    let green = data[(addr + 1) as usize];
                    let blue = data[(addr + 2) as usize];
                    let mut color = Color::Rgb(red, green, blue);
                    color.shift(drift);
                    self.set_pixel(x as u32, y as u32, color);
                }
            }
        }
    }

    fn update(&mut self, _rect: &Rectangle, _mode: UpdateMode) -> Result<u32, Error> {
        self.0.present();
        Ok(Local::now().timestamp_subsec_millis())
    }

    fn wait(&self, _tok: u32) -> Result<i32, Error> {
        Ok(1)
    }

    fn save(&self, path: &str) -> Result<(), Error> {
        let (width, height) = self.dims();
        let file =
            File::create(path).with_context(|| format!("can't create output file {}", path))?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder
            .write_header()
            .with_context(|| format!("can't write PNG header for {}", path))?;
        let data = self
            .0
            .read_pixels(self.0.viewport(), PixelFormatEnum::RGB24)
            .unwrap_or_default();
        writer
            .write_image_data(&data)
            .with_context(|| format!("can't write PNG data to {}", path))?;
        Ok(())
    }

    fn rotation(&self) -> i8 {
        DEFAULT_ROTATION
    }

    fn set_rotation(&mut self, n: i8) -> Result<(u32, u32), Error> {
        let (mut width, mut height) = self.dims();
        if (width < height && n % 2 == 0) || (width > height && n % 2 == 1) {
            mem::swap(&mut width, &mut height);
        }
        self.0.window_mut().set_size(width, height).ok();
        Ok((width, height))
    }

    fn set_monochrome(&mut self, _enable: bool) {}

    fn set_dithered(&mut self, _enable: bool) {}

    fn set_inverted(&mut self, _enable: bool) {}

    fn monochrome(&self) -> bool {
        false
    }

    fn dithered(&self) -> bool {
        false
    }

    fn inverted(&self) -> bool {
        false
    }

    fn width(&self) -> u32 {
        self.0.window().size().0
    }

    fn height(&self) -> u32 {
        self.0.window().size().1
    }
}

fn main() -> Result<(), Error> {
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    let (width, height) = CURRENT_DEVICE.dims;
    let window = video_subsystem
        .window("Cadmus Emulator", width, height)
        .position_centered()
        .build()
        .unwrap();

    let mut fb = window.into_canvas().software().build().unwrap();
    fb.set_blend_mode(BlendMode::Blend);

    let mut context = build_context(Box::new(FBCanvas(fb)))?;

    if context.settings.import.startup_trigger {
        context.batch_import();
    }

    context.load_dictionaries();
    context.load_keyboard_layouts();

    let (tx, rx) = mpsc::channel();
    let (ty, ry) = mpsc::channel();
    let touch_screen = gesture_events(ry);

    let tx2 = tx.clone();
    thread::spawn(move || {
        while let Ok(evt) = touch_screen.recv() {
            tx2.send(evt).ok();
        }
    });

    let tx3 = tx.clone();
    thread::spawn(move || loop {
        thread::sleep(CLOCK_REFRESH_INTERVAL);
        tx3.send(Event::ClockTick).ok();
    });

    let mut history: Vec<Box<dyn View>> = Vec::new();
    let mut rq = RenderQueue::new();
    let mut view: Box<dyn View> =
        Box::new(Home::new(context.fb.rect(), &tx, &mut rq, &mut context)?);

    let mut updating = Vec::new();

    if context.settings.frontlight {
        let levels = context.settings.frontlight_levels;
        context.frontlight.set_intensity(levels.intensity);
        context.frontlight.set_warmth(levels.warmth);
    } else {
        context.frontlight.set_warmth(0.0);
        context.frontlight.set_intensity(0.0);
    }

    info!(
        "{} is running on a Kobo {}.",
        APP_NAME, CURRENT_DEVICE.model
    );
    info!(
        "The framebuffer resolution is {} by {}.",
        context.fb.rect().width(),
        context.fb.rect().height()
    );

    let mut bus = VecDeque::with_capacity(4);

    'outer: loop {
        let mut event_pump = sdl_context.event_pump().unwrap();
        while let Some(sdl_evt) = event_pump.poll_event() {
            match sdl_evt {
                SdlEvent::Quit { .. }
                | SdlEvent::KeyDown {
                    keycode: Some(Keycode::Escape),
                    keymod: Mod::NOMOD,
                    ..
                } => {
                    view.handle_event(
                        &Event::Back,
                        &tx,
                        &mut VecDeque::new(),
                        &mut RenderQueue::new(),
                        &mut context,
                    );
                    while let Some(mut view) = history.pop() {
                        view.handle_event(
                            &Event::Back,
                            &tx,
                            &mut VecDeque::new(),
                            &mut RenderQueue::new(),
                            &mut context,
                        );
                    }
                    break 'outer;
                }
                SdlEvent::KeyUp {
                    scancode: Some(scancode),
                    keymod: Mod::NOMOD,
                    timestamp,
                    ..
                } => {
                    if let Some(code) = code_from_key(scancode) {
                        ty.send(DeviceEvent::Button {
                            time: seconds(timestamp),
                            code,
                            status: ButtonStatus::Released,
                        })
                        .ok();
                    }
                }
                SdlEvent::KeyDown {
                    scancode: Some(scancode),
                    keymod,
                    timestamp,
                    repeat,
                    ..
                } => match keymod {
                    Mod::NOMOD => match scancode {
                        Scancode::LeftBracket => {
                            let rot = (3 + context.display.rotation) % 4;
                            ty.send(DeviceEvent::RotateScreen(rot)).ok();
                        }
                        Scancode::RightBracket => {
                            let rot = (5 + context.display.rotation) % 4;
                            ty.send(DeviceEvent::RotateScreen(rot)).ok();
                        }
                        Scancode::S => {
                            tx.send(Event::Select(EntryId::TakeScreenshot)).ok();
                        }
                        Scancode::B
                        | Scancode::F
                        | Scancode::P
                        | Scancode::L
                        | Scancode::H
                        | Scancode::E
                        | Scancode::G => {
                            if let Some(code) = code_from_key(scancode) {
                                let status = if repeat {
                                    ButtonStatus::Repeated
                                } else {
                                    ButtonStatus::Pressed
                                };
                                ty.send(DeviceEvent::Button {
                                    time: seconds(timestamp),
                                    code,
                                    status,
                                })
                                .ok();
                            }
                        }
                        Scancode::I | Scancode::O => {
                            let mouse_state = MouseState::new(&event_pump);
                            let x = mouse_state.x() as i32;
                            let y = mouse_state.y() as i32;
                            let center = pt!(x, y);
                            if scancode == Scancode::I {
                                tx.send(Event::Gesture(GestureEvent::Spread {
                                    center,
                                    factor: 2.0,
                                    axis: Axis::Diagonal,
                                }))
                                .ok();
                            } else {
                                tx.send(Event::Gesture(GestureEvent::Pinch {
                                    center,
                                    factor: 0.5,
                                    axis: Axis::Diagonal,
                                }))
                                .ok();
                            }
                        }
                        _ => (),
                    },
                    Mod::LSHIFTMOD | Mod::RSHIFTMOD => match scancode {
                        Scancode::S | Scancode::P | Scancode::C => {
                            if let Some(index) = locate::<Intermission>(view.as_ref()) {
                                let rect = *view.child(index).rect();
                                view.children_mut().remove(index);
                                rq.add(RenderData::expose(rect, UpdateMode::Full));
                            } else {
                                view.handle_event(
                                    &Event::Suspend,
                                    &tx,
                                    &mut VecDeque::new(),
                                    &mut RenderQueue::new(),
                                    &mut context,
                                );
                                let kind = match scancode {
                                    Scancode::S => IntermKind::Suspend,
                                    Scancode::P => IntermKind::PowerOff,
                                    Scancode::C => IntermKind::Share,
                                    _ => unreachable!(),
                                };
                                let interm = Intermission::new(context.fb.rect(), kind, &context);
                                rq.add(RenderData::new(
                                    interm.id(),
                                    *interm.rect(),
                                    UpdateMode::Full,
                                ));
                                view.children_mut().push(Box::new(interm) as Box<dyn View>);
                            }
                        }
                        _ => (),
                    },
                    _ => (),
                },
                _ => {
                    if let Some(dev_evt) = device_event(sdl_evt) {
                        ty.send(dev_evt).ok();
                    }
                }
            }
        }

        while let Ok(evt) = rx.recv_timeout(Duration::from_millis(20)) {
            match evt {
                Event::Open(info) => {
                    let rotation = context.display.rotation;
                    if let Some(n) = info
                        .reader
                        .as_ref()
                        .and_then(|r| r.rotation.map(|n| CURRENT_DEVICE.from_canonical(n)))
                    {
                        if n != rotation {
                            if let Ok(dims) = context.fb.set_rotation(n) {
                                context.display.rotation = n;
                                context.display.dims = dims;
                            }
                        }
                    }
                    let path = info.file.path.clone();
                    if let Some(r) = Reader::new(context.fb.rect(), *info, &tx, &mut context) {
                        let mut next_view = Box::new(r) as Box<dyn View>;
                        transfer_notifications(
                            view.as_mut(),
                            next_view.as_mut(),
                            &mut rq,
                            &mut context,
                        );
                        history.push(view as Box<dyn View>);
                        view = next_view;
                    } else {
                        if context.display.rotation != rotation {
                            if let Ok(dims) = context.fb.set_rotation(rotation) {
                                context.display.rotation = rotation;
                                context.display.dims = dims;
                            }
                        }
                        handle_event(
                            view.as_mut(),
                            &Event::Invalid(path),
                            &tx,
                            &mut bus,
                            &mut rq,
                            &mut context,
                        );
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
                    transfer_notifications(
                        view.as_mut(),
                        next_view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    history.push(view as Box<dyn View>);
                    view = next_view;
                }
                Event::Select(EntryId::Launch(app_cmd)) => {
                    view.children_mut().retain(|child| !child.is::<Menu>());
                    let mut next_view: Box<dyn View> = match app_cmd {
                        AppCmd::Sketch => {
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
                        } => Box::new(Dictionary::new(
                            context.fb.rect(),
                            query,
                            language,
                            &tx,
                            &mut rq,
                            &mut context,
                        )),
                        AppCmd::SettingsEditor => Box::new(SettingsEditor::new(
                            context.fb.rect(),
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
                    };
                    transfer_notifications(
                        view.as_mut(),
                        next_view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    history.push(view as Box<dyn View>);
                    view = next_view;
                }
                Event::Select(EntryId::CheckForUpdates) => {
                    tracing::trace!("showing OTA view");
                    show_ota_view(view.as_mut(), &tx, &mut rq, &mut context);
                }
                Event::Back => {
                    if let Some(v) = history.pop() {
                        view = v;
                        if view.is::<Home>() {
                            if context.display.rotation % 2 != 1 {
                                if let Ok(dims) = context.fb.set_rotation(DEFAULT_ROTATION) {
                                    context.display.rotation = DEFAULT_ROTATION;
                                    context.display.dims = dims;
                                }
                            }
                        }
                        view.handle_event(&Event::Reseed, &tx, &mut bus, &mut rq, &mut context);
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
                Event::Select(EntryId::About) => {
                    let dialog = Dialog::builder(
                        ViewId::AboutDialog,
                        format!("Cadmus {}", env!("GIT_VERSION")),
                    )
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
                    transfer_notifications(
                        view.as_mut(),
                        next_view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    history.push(view as Box<dyn View>);
                    view = next_view;
                }
                Event::Select(EntryId::OpenDocumentation) => {
                    view.children_mut().retain(|child| !child.is::<Menu>());

                    let r = open_documentation(context.fb.rect(), &tx, &mut context)
                        .expect("Failed to open documentation");

                    let mut next_view = Box::new(r) as Box<dyn View>;
                    transfer_notifications(
                        view.as_mut(),
                        next_view.as_mut(),
                        &mut rq,
                        &mut context,
                    );
                    history.push(view as Box<dyn View>);
                    view = next_view;
                }
                Event::Select(EntryId::Rotate(n))
                    if n != context.display.rotation && view.might_rotate() =>
                {
                    wait_for_all(&mut updating, &mut context);
                    if let Ok(dims) = context.fb.set_rotation(n) {
                        context.display.rotation = n;
                        let fb_rect = Rectangle::from(dims);
                        if context.display.dims != dims {
                            context.display.dims = dims;
                            view.resize(fb_rect, &tx, &mut rq, &mut context);
                        }
                    }
                }
                Event::Select(EntryId::SetButtonScheme(button_scheme)) => {
                    context.settings.button_scheme = button_scheme;

                    // Re-dispatch event to view hierarchy so UI can update
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
                }
                Event::Select(EntryId::ToggleInverted) => {
                    context.fb.toggle_inverted();
                    rq.add(RenderData::new(
                        view.id(),
                        context.fb.rect(),
                        UpdateMode::Gui,
                    ));
                }
                Event::Select(EntryId::TakeScreenshot) => {
                    let name = Local::now().format("screenshot-%Y%m%d_%H%M%S.png");
                    let msg = match context.fb.save(&name.to_string()) {
                        Err(e) => format!("Couldn't take screenshot: {}).", e),
                        Ok(_) => format!("Saved {}.", name),
                    };
                    let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                }
                Event::Notification(notif_event) => match notif_event {
                    NotificationEvent::Show(msg) => {
                        let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                        view.children_mut().push(Box::new(notif) as Box<dyn View>);
                    }
                    NotificationEvent::ShowPinned(id, msg) => {
                        let notif =
                            Notification::new(Some(id), msg, true, &tx, &mut rq, &mut context);
                        view.children_mut().push(Box::new(notif) as Box<dyn View>);
                    }
                    NotificationEvent::UpdateText(id, text) => {
                        if let Some(notif) = find_notification_mut(view.as_mut(), id) {
                            notif.update_text(text, &mut rq);
                        } else {
                            panic!(
                                "Attempted to update non-existent notification with id: {:?}",
                                id
                            );
                        }
                    }
                    NotificationEvent::UpdateProgress(id, progress) => {
                        if let Some(notif) = find_notification_mut(view.as_mut(), id) {
                            notif.update_progress(progress, &mut rq);
                        } else {
                            panic!(
                                "Attempted to update progress of non-existent notification with id: {:?}",
                                id
                            );
                        }
                    }
                },
                Event::Notify(msg) => {
                    let notif = Notification::new(None, msg, false, &tx, &mut rq, &mut context);
                    view.children_mut().push(Box::new(notif) as Box<dyn View>);
                }
                Event::Device(DeviceEvent::NetUp)
                | Event::CheckFetcher(..)
                | Event::FetcherAddDocument(..)
                | Event::FetcherRemoveDocument(..)
                | Event::FetcherSearch { .. }
                    if !view.is::<Home>() =>
                {
                    if let Some(home) = history.get_mut(0).filter(|view| view.is::<Home>()) {
                        let (tx, _rx) = mpsc::channel();
                        home.handle_event(
                            &evt,
                            &tx,
                            &mut VecDeque::new(),
                            &mut RenderQueue::new(),
                            &mut context,
                        );
                    }
                }
                Event::SetWifi(enable) => {
                    if context.settings.wifi != enable {
                        context.settings.wifi = enable;
                        if enable {
                            let tx2 = tx.clone();
                            thread::spawn(move || {
                                thread::sleep(Duration::from_secs(2));
                                tx2.send(Event::Device(DeviceEvent::NetUp)).ok();
                            });
                        } else {
                            context.online = false;
                        }
                    }
                }
                Event::Device(DeviceEvent::RotateScreen(n)) => {
                    tx.send(Event::Select(EntryId::Rotate(n))).ok();
                }
                Event::Select(EntryId::Quit) => {
                    break 'outer;
                }
                _ => {
                    handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
                }
            }
        }

        process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);

        while let Some(ce) = bus.pop_front() {
            tx.send(ce).ok();
        }
    }

    if !history.is_empty() {
        let (tx, _rx) = mpsc::channel();
        view.handle_event(
            &Event::Back,
            &tx,
            &mut VecDeque::new(),
            &mut RenderQueue::new(),
            &mut context,
        );
        while let Some(mut view) = history.pop() {
            view.handle_event(
                &Event::Back,
                &tx,
                &mut VecDeque::new(),
                &mut RenderQueue::new(),
                &mut context,
            );
        }
    }

    if context.settings.frontlight {
        context.settings.frontlight_levels = context.frontlight.levels();
    }

    context.library.flush();

    let path = Path::new(SETTINGS_PATH);
    save_toml(&context.settings, path).context("can't save settings")?;

    cadmus_core::logging::shutdown_logging();

    Ok(())
}
