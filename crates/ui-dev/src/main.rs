mod view;

use cadmus_core::anyhow::{Context as ResultExt, Error};
use cadmus_core::battery::{Battery, FakeBattery};
use cadmus_core::chrono::Local;
use cadmus_core::color::Color;
use cadmus_core::context::Context;
use cadmus_core::device::CURRENT_DEVICE;
use cadmus_core::font::Fonts;
use cadmus_core::framebuffer::{Framebuffer, UpdateMode};
use cadmus_core::frontlight::{Frontlight, LightLevels};
use cadmus_core::geom::Rectangle;
use cadmus_core::gesture::gesture_events;
use cadmus_core::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use cadmus_core::library::Library;
use cadmus_core::lightsensor::LightSensor;
use cadmus_core::png;
use cadmus_core::pt;
use cadmus_core::settings::{IntermKind, Settings};
use cadmus_core::view::common::locate;
use cadmus_core::view::intermission::Intermission;
use cadmus_core::view::notification::Notification;
use cadmus_core::view::{Event, RenderData, RenderQueue, View, handle_event, process_render_queue};
use sdl2::event::Event as SdlEvent;
use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::pixels::{Color as SdlColor, PixelFormatEnum};
use sdl2::rect::Point as SdlPoint;
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, WindowCanvas};
use std::collections::VecDeque;
use std::fs::File;
use std::mem;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub const APP_NAME: &str = "Plato UI Dev";
const DEFAULT_ROTATION: i8 = 1;

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

    fn save(&self, _path: &str) -> Result<(), Error> {
        unimplemented!()
    }

    fn rotation(&self) -> i8 {
        DEFAULT_ROTATION
    }

    fn set_rotation(&mut self, _n: i8) -> Result<(u32, u32), Error> {
        unimplemented!()
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

pub fn build_context(fb: Box<dyn Framebuffer>) -> Result<Context, Error> {
    let mut settings = Settings::default();

    settings.libraries[0].path = PathBuf::from(".");

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

fn main() -> Result<(), Error> {
    let sdl_context = sdl2::init().unwrap();
    let video_subsystem = sdl_context.video().unwrap();
    let (width, height) = CURRENT_DEVICE.dims;
    let window = video_subsystem
        .window("Plato UI Dev", width, height)
        .position_centered()
        .build()
        .unwrap();

    let mut fb = window.into_canvas().software().build().unwrap();
    fb.set_blend_mode(BlendMode::Blend);

    let mut context = build_context(Box::new(FBCanvas(fb)))?;

    let (tx, rx) = mpsc::channel();
    let (ty, ry) = mpsc::channel();
    let touch_screen = gesture_events(ry);

    let tx2 = tx.clone();
    thread::spawn(move || {
        while let Ok(evt) = touch_screen.recv() {
            tx2.send(evt).ok();
        }
    });

    let mut rq = RenderQueue::new();
    let mut view: Box<dyn View> =
        view::create_root_view(context.fb.rect(), &tx, &mut rq, &mut context);

    rq.add(RenderData::new(view.id(), *view.rect(), UpdateMode::Full));

    let mut updating = Vec::new();

    if context.settings.frontlight {
        let levels = context.settings.frontlight_levels;
        context.frontlight.set_intensity(levels.intensity);
        context.frontlight.set_warmth(levels.warmth);
    } else {
        context.frontlight.set_warmth(0.0);
        context.frontlight.set_intensity(0.0);
    }

    println!(
        "{} is running on a Kobo {}.",
        APP_NAME, CURRENT_DEVICE.model
    );
    println!(
        "The framebuffer resolution is {} by {}.",
        context.fb.rect().width(),
        context.fb.rect().height()
    );

    let mut bus = VecDeque::with_capacity(4);

    process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);

    'outer: loop {
        let mut event_pump = sdl_context.event_pump().unwrap();
        while let Some(sdl_evt) = event_pump.poll_event() {
            match sdl_evt {
                SdlEvent::Quit { .. } => {
                    break 'outer;
                }
                _ => {
                    if let Some(dev_evt) = device_event(sdl_evt) {
                        ty.send(dev_evt).ok();
                    }
                }
            }
        }

        while let Ok(evt) = rx.recv_timeout(Duration::from_millis(20)) {
            match evt {
                Event::Device(DeviceEvent::RotateScreen(n)) => {
                    if n != context.display.rotation {
                        if let Ok(dims) = context.fb.set_rotation(n) {
                            context.display.rotation = n;
                            let fb_rect = Rectangle::from(dims);
                            if context.display.dims != dims {
                                context.display.dims = dims;
                                view.resize(fb_rect, &tx, &mut rq, &mut context);
                            }
                        }
                    }
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

    Ok(())
}
