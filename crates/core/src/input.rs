use crate::framebuffer::Display;
use crate::geom::{LinearDir, Point};
use crate::settings::ButtonScheme;
use anyhow::{Context, Error};
use fxhash::FxHashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::mem::{self, MaybeUninit};
use std::os::unix::io::AsRawFd;
use std::ptr;
use std::slice;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

// Event types
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;

// Event codes
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_PRESSURE: u16 = 0x3a;
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;
pub const MSC_RAW: u16 = 0x03;
pub const SYN_REPORT: u16 = 0x00;

// Event values
pub const MSC_RAW_GSENSOR_PORTRAIT_DOWN: i32 = 0x17;
pub const MSC_RAW_GSENSOR_PORTRAIT_UP: i32 = 0x18;
pub const MSC_RAW_GSENSOR_LANDSCAPE_RIGHT: i32 = 0x19;
pub const MSC_RAW_GSENSOR_LANDSCAPE_LEFT: i32 = 0x1a;
// pub const MSC_RAW_GSENSOR_BACK: i32 = 0x1b;
// pub const MSC_RAW_GSENSOR_FRONT: i32 = 0x1c;

// The indices of this clockwise ordering of the sensor values match the Forma's rotation values.
pub const GYROSCOPE_ROTATIONS: [i32; 4] = [
    MSC_RAW_GSENSOR_LANDSCAPE_LEFT,
    MSC_RAW_GSENSOR_PORTRAIT_UP,
    MSC_RAW_GSENSOR_LANDSCAPE_RIGHT,
    MSC_RAW_GSENSOR_PORTRAIT_DOWN,
];

pub const VAL_RELEASE: i32 = 0;
pub const VAL_PRESS: i32 = 1;
pub const VAL_REPEAT: i32 = 2;

// Key codes
pub const KEY_POWER: u16 = 116;
pub const KEY_HOME: u16 = 102;
pub const KEY_LIGHT: u16 = 90;
pub const KEY_BACKWARD: u16 = 193;
pub const KEY_FORWARD: u16 = 194;
pub const PEN_ERASE: u16 = 331;
pub const PEN_HIGHLIGHT: u16 = 332;
pub const SLEEP_COVER: [u16; 2] = [59, 35];
// Synthetic touch button
pub const BTN_TOUCH: u16 = 330;
// The following key codes are fake, and are used to support
// software toggles within this design
pub const KEY_ROTATE_DISPLAY: u16 = 0xffff;
pub const KEY_BUTTON_SCHEME: u16 = 0xfffe;

pub const SINGLE_TOUCH_CODES: TouchCodes = TouchCodes {
    pressure: ABS_PRESSURE,
    x: ABS_X,
    y: ABS_Y,
};

pub const MULTI_TOUCH_CODES_A: TouchCodes = TouchCodes {
    pressure: ABS_MT_TOUCH_MAJOR,
    x: ABS_MT_POSITION_X,
    y: ABS_MT_POSITION_Y,
};

pub const MULTI_TOUCH_CODES_B: TouchCodes = TouchCodes {
    pressure: ABS_MT_PRESSURE,
    ..MULTI_TOUCH_CODES_A
};

#[repr(C)]
#[derive(Debug)]
pub struct InputEvent {
    pub time: libc::timeval,
    pub kind: u16, // type
    pub code: u16,
    pub value: i32,
}

// Handle different touch protocols
#[derive(Debug)]
pub struct TouchCodes {
    pressure: u16,
    x: u16,
    y: u16,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TouchProto {
    Single,
    MultiA,
    MultiB, // Pressure won't indicate a finger release.
    MultiC,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FingerStatus {
    Down,
    Motion,
    Up,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ButtonStatus {
    Pressed,
    Released,
    Repeated,
}

impl ButtonStatus {
    pub fn try_from_raw(value: i32) -> Option<ButtonStatus> {
        match value {
            VAL_RELEASE => Some(ButtonStatus::Released),
            VAL_PRESS => Some(ButtonStatus::Pressed),
            VAL_REPEAT => Some(ButtonStatus::Repeated),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum ButtonCode {
    Power,
    Home,
    Light,
    Backward,
    Forward,
    Erase,
    Highlight,
    Raw(u16),
}

impl ButtonCode {
    fn from_raw(
        code: u16,
        rotation: i8,
        button_scheme: ButtonScheme,
        startup_rotation: i8,
        dir: i8,
    ) -> ButtonCode {
        match code {
            KEY_POWER => ButtonCode::Power,
            KEY_HOME => ButtonCode::Home,
            KEY_LIGHT => ButtonCode::Light,
            KEY_BACKWARD => resolve_button_direction(
                LinearDir::Backward,
                rotation,
                button_scheme,
                startup_rotation,
                dir,
            ),
            KEY_FORWARD => resolve_button_direction(
                LinearDir::Forward,
                rotation,
                button_scheme,
                startup_rotation,
                dir,
            ),
            PEN_ERASE => ButtonCode::Erase,
            PEN_HIGHLIGHT => ButtonCode::Highlight,
            _ => ButtonCode::Raw(code),
        }
    }
}

fn resolve_button_direction(
    mut direction: LinearDir,
    rotation: i8,
    button_scheme: ButtonScheme,
    startup_rotation: i8,
    dir: i8,
) -> ButtonCode {
    let should_invert = rotation == (4 + startup_rotation - dir) % 4
        || rotation == (4 + startup_rotation - 2 * dir) % 4;
    if should_invert ^ (button_scheme == ButtonScheme::Inverted) {
        direction = direction.opposite();
    }

    if direction == LinearDir::Forward {
        return ButtonCode::Forward;
    }

    ButtonCode::Backward
}

pub fn display_rotate_event(n: i8) -> InputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    InputEvent {
        time: tp,
        kind: EV_KEY,
        code: KEY_ROTATE_DISPLAY,
        value: n as i32,
    }
}

pub fn button_scheme_event(v: i32) -> InputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    InputEvent {
        time: tp,
        kind: EV_KEY,
        code: KEY_BUTTON_SCHEME,
        value: v,
    }
}

#[derive(Debug, Copy, Clone)]
pub enum DeviceEvent {
    Finger {
        id: i32,
        time: f64,
        status: FingerStatus,
        position: Point,
    },
    Button {
        time: f64,
        code: ButtonCode,
        status: ButtonStatus,
    },
    Plug(PowerSource),
    Unplug(PowerSource),
    /// Screen rotation request (`0`, `90`, `180`, or `270` degrees).
    ///
    /// On Kobo, handled by the lifecycle path to rotate the framebuffer. In the
    /// emulator lifecycle, the event is swallowed because rotation is
    /// unsupported, avoiding partial state.
    RotateScreen(i8),
    CoverOn,
    CoverOff,
    /// Network interface is up (DHCP complete).
    ///
    /// Emitted when WiFi monitoring detects a completed interface binding.
    ///
    /// Dispatch is two-phase:
    ///
    /// 1. **Lifecycle** — On Kobo, the lifecycle `handle_net_up` handler sets
    ///    `context.online`, shows a notification, and returns
    ///    [`EventOutcome::Continue`](crate::device::EventOutcome) when the
    ///    network was previously offline.
    /// 2. **Main loop** — The application main loop forwards the event to the
    ///    background [`Home`](crate::view::home::Home) view (history slot 0) when
    ///    another screen is active, so library fetchers can resume.
    NetUp,
    UserActivity,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum PowerSource {
    Host,
    Wall,
}

pub fn seconds(time: libc::timeval) -> f64 {
    time.tv_sec as f64 + time.tv_usec as f64 / 1e6
}

pub fn raw_events(paths: Vec<String>) -> (Sender<InputEvent>, Receiver<InputEvent>) {
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    thread::spawn(move || parse_raw_events(&paths, &tx));
    (tx2, rx)
}

pub fn parse_raw_events(paths: &[String], tx: &Sender<InputEvent>) -> Result<(), Error> {
    let mut files = Vec::new();
    let mut pfds = Vec::new();

    for path in paths.iter() {
        let file = File::open(path).with_context(|| format!("can't open input file {}", path))?;
        let fd = file.as_raw_fd();
        files.push(file);
        pfds.push(libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
    }

    loop {
        let ret = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, -1) };
        if ret < 0 {
            break;
        }
        for (pfd, mut file) in pfds.iter().zip(&files) {
            if pfd.revents & libc::POLLIN != 0 {
                let mut input_event = MaybeUninit::<InputEvent>::uninit();
                unsafe {
                    let event_slice = slice::from_raw_parts_mut(
                        input_event.as_mut_ptr() as *mut u8,
                        mem::size_of::<InputEvent>(),
                    );
                    if file.read_exact(event_slice).is_err() {
                        break;
                    }
                    tx.send(input_event.assume_init()).ok();
                }
            }
        }
    }

    Ok(())
}

pub fn usb_events() -> Receiver<DeviceEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || parse_usb_events(&tx));
    rx
}

fn parse_usb_events(tx: &Sender<DeviceEvent>) {
    let path = CString::new("/tmp/nickel-hardware-status").unwrap();
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_NONBLOCK | libc::O_RDWR) };

    if fd < 0 {
        return;
    }

    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };

    const BUF_LEN: usize = 256;

    loop {
        let ret = unsafe { libc::poll(&mut pfd as *mut libc::pollfd, 1, -1) };

        if ret < 0 {
            break;
        }

        let buf = CString::new(vec![1; BUF_LEN]).unwrap();
        let c_buf = buf.into_raw();

        if pfd.revents & libc::POLLIN != 0 {
            let n = unsafe { libc::read(fd, c_buf as *mut libc::c_void, BUF_LEN as libc::size_t) };
            let buf = unsafe { CString::from_raw(c_buf) };
            if n > 0 {
                if let Ok(s) = buf.to_str() {
                    for msg in s[..n as usize].lines() {
                        if msg == "usb plug add" {
                            tx.send(DeviceEvent::Plug(PowerSource::Host)).ok();
                        } else if msg == "usb plug remove" {
                            tx.send(DeviceEvent::Unplug(PowerSource::Host)).ok();
                        } else if msg == "usb ac add" {
                            tx.send(DeviceEvent::Plug(PowerSource::Wall)).ok();
                        } else if msg == "usb ac remove" {
                            tx.send(DeviceEvent::Unplug(PowerSource::Wall)).ok();
                        }
                    }
                }
            } else {
                break;
            }
        }
    }
}

fn compute_mirror_axes(rotation: i8, mirroring_scheme: (i8, i8)) -> (bool, bool) {
    let (mxy, dir) = mirroring_scheme;
    let mx = (4 + (mxy + dir)) % 4;
    let my = (4 + (mxy - dir)) % 4;
    let mirror_x = mxy == rotation || mx == rotation;
    let mirror_y = mxy == rotation || my == rotation;
    (mirror_x, mirror_y)
}

/// Captures the subset of device properties needed for input event processing.
#[derive(Clone, Copy)]
pub struct DeviceInputInfo {
    pub proto: TouchProto,
    pub mark: u8,
    pub mirroring_scheme: (i8, i8),
    pub swapping_scheme: i8,
    pub startup_rotation: i8,
    pub gyro_rotation_transform: GyroRotationTransform,
    /// When true, the input thread swaps logical screen dimensions on 90° rotations.
    /// KoboFramebuffer2 does this in hardware; KoboFramebuffer1 does not.
    pub swap_dims_on_rotation: bool,
}

#[derive(Clone, Copy)]
pub struct GyroRotationTransform(fn(i8) -> i8);

impl GyroRotationTransform {
    pub fn new(f: fn(i8) -> i8) -> Self {
        Self(f)
    }

    pub fn transform(&self, n: i8) -> i8 {
        (self.0)(n)
    }
}

impl Default for GyroRotationTransform {
    fn default() -> Self {
        Self(|n| n)
    }
}

pub fn device_events(
    rx: Receiver<InputEvent>,
    display: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) -> Receiver<DeviceEvent> {
    let Display { dims, rotation } = display;
    tracing::trace!(
        rotation,
        screen_dims = ?dims,
        proto = ?info.proto,
        mark = info.mark,
        mirroring_scheme = ?info.mirroring_scheme,
        swapping_scheme = info.swapping_scheme,
        startup_rotation = info.startup_rotation,
        "starting device event pipeline"
    );
    let (ty, ry) = mpsc::channel();
    thread::spawn(move || {
        parse_device_events(&rx, &ty, Display { dims, rotation }, button_scheme, info)
    });
    ry
}

#[repr(C)]
#[derive(Debug)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

fn evdev_abs_info(path: &str, axis: u16) -> Option<InputAbsInfo> {
    let file = File::open(path).ok()?;
    let fd = file.as_raw_fd();
    let mut absinfo = MaybeUninit::<InputAbsInfo>::uninit();
    let request = evdev_abs_ioctl(axis);
    let ret = unsafe { libc::ioctl(fd, request as libc::c_ulong, absinfo.as_mut_ptr()) };
    if ret < 0 {
        return None;
    }
    Some(unsafe { absinfo.assume_init() })
}

const fn evdev_abs_ioctl(axis: u16) -> u32 {
    const IOC_READ: u32 = 2;
    const IOC_TYPE_E: u32 = b'E' as u32;
    const ABSINFO_SIZE: u32 = mem::size_of::<InputAbsInfo>() as u32;
    (IOC_READ << 30) | (ABSINFO_SIZE << 16) | (IOC_TYPE_E << 8) | (0x40 + axis as u32)
}

pub fn gsensor_rotation_from_raw(value: i32, transform: GyroRotationTransform) -> Option<i8> {
    if !(MSC_RAW_GSENSOR_PORTRAIT_DOWN..=MSC_RAW_GSENSOR_LANDSCAPE_LEFT).contains(&value) {
        return None;
    }
    GYROSCOPE_ROTATIONS
        .iter()
        .position(|&v| v == value)
        .map(|i| transform.transform(i as i8))
}

pub fn trace_touch_device_geometry(path: &str, proto: TouchProto, screen: Display) {
    let Display { dims, rotation } = screen;
    let (x_axis, y_axis) = match proto {
        TouchProto::Single => (ABS_X, ABS_Y),
        TouchProto::MultiA | TouchProto::MultiB | TouchProto::MultiC => {
            (ABS_MT_POSITION_X, ABS_MT_POSITION_Y)
        }
    };
    let abs_x = evdev_abs_info(path, x_axis);
    let abs_y = evdev_abs_info(path, y_axis);
    tracing::trace!(
        path,
        proto = ?proto,
        screen_rotation = rotation,
        screen_dims = ?dims,
        abs_x = ?abs_x,
        abs_y = ?abs_y,
        "evdev vs display geometry"
    );
}

#[derive(Debug)]
struct TouchState {
    position: Point,
    pressure: i32,
}

impl Default for TouchState {
    fn default() -> Self {
        TouchState {
            position: Point::default(),
            pressure: 0,
        }
    }
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(rx, ty, display, button_scheme, info),
        level = tracing::Level::TRACE,
    )
)]
pub fn parse_device_events(
    rx: &Receiver<InputEvent>,
    ty: &Sender<DeviceEvent>,
    display: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) {
    let DeviceInputInfo {
        proto,
        mark,
        mirroring_scheme,
        swapping_scheme,
        startup_rotation,
        gyro_rotation_transform,
        swap_dims_on_rotation,
    } = info;
    let (_, dir) = mirroring_scheme;
    let mut id = 0;
    let mut last_activity = -60;
    let Display {
        mut dims,
        mut rotation,
    } = display;
    let mut fingers: FxHashMap<i32, Point> = FxHashMap::default();
    let mut packets: FxHashMap<i32, TouchState> = FxHashMap::default();

    let mut tc = match proto {
        TouchProto::Single => SINGLE_TOUCH_CODES,
        TouchProto::MultiA => MULTI_TOUCH_CODES_A,
        TouchProto::MultiB => MULTI_TOUCH_CODES_B,
        TouchProto::MultiC => MULTI_TOUCH_CODES_B,
    };

    if proto == TouchProto::Single {
        packets.insert(id, TouchState::default());
    }

    let (mut mirror_x, mut mirror_y) = compute_mirror_axes(rotation, mirroring_scheme);
    if rotation % 2 == swapping_scheme {
        mem::swap(&mut tc.x, &mut tc.y);
    }

    let axes_swapped = rotation % 2 == swapping_scheme;
    tracing::trace!(
        rotation,
        dims = ?dims,
        mirror_x,
        mirror_y,
        mirroring_scheme = ?mirroring_scheme,
        swapping_scheme,
        startup_rotation,
        swap_dims_on_rotation,
        proto = ?proto,
        mark,
        tc_x = tc.x,
        tc_y = tc.y,
        axes_swapped,
        "parse_device_events started"
    );

    let mut button_scheme = button_scheme;

    while let Ok(evt) = rx.recv() {
        let _span = tracing::trace_span!("processing input event", event = ?evt).entered();

        if evt.kind == EV_ABS {
            if evt.code == ABS_MT_TRACKING_ID {
                if evt.value >= 0 {
                    id = evt.value;
                    packets.insert(id, TouchState::default());
                }
            } else if evt.code == tc.x {
                if let Some(state) = packets.get_mut(&id) {
                    state.position.x = if mirror_x {
                        dims.0 as i32 - 1 - evt.value
                    } else {
                        evt.value
                    };
                    tracing::trace!(
                        raw = evt.value,
                        evt_code = evt.code,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axis = "screen_x",
                        mirrored = mirror_x,
                        dim_used = dims.0,
                        result = state.position.x,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        "touch axis raw"
                    );
                }
            } else if evt.code == tc.y {
                if let Some(state) = packets.get_mut(&id) {
                    state.position.y = if mirror_y {
                        dims.1 as i32 - 1 - evt.value
                    } else {
                        evt.value
                    };
                    tracing::trace!(
                        raw = evt.value,
                        evt_code = evt.code,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axis = "screen_y",
                        mirrored = mirror_y,
                        dim_used = dims.1,
                        result = state.position.y,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        "touch axis raw"
                    );
                }
            } else if evt.code == tc.pressure {
                if let Some(state) = packets.get_mut(&id) {
                    state.pressure = evt.value;
                    if proto == TouchProto::Single && mark == 3 && state.pressure == 0 {
                        state.position.x = dims.0 as i32 - 1 - state.position.x;
                        mem::swap(&mut state.position.x, &mut state.position.y);
                    }
                }
            }
        } else if evt.kind == EV_SYN && evt.code == SYN_REPORT {
            // The absolute value accounts for the wrapping around that might occur,
            // since `tv_sec` can't grow forever.
            if (evt.time.tv_sec - last_activity).abs() >= 60 {
                last_activity = evt.time.tv_sec;
                ty.send(DeviceEvent::UserActivity).ok();
            }

            if proto == TouchProto::MultiB {
                fingers.retain(|other_id, other_position| {
                    packets.contains_key(&other_id)
                        || ty
                            .send(DeviceEvent::Finger {
                                id: *other_id,
                                time: seconds(evt.time),
                                status: FingerStatus::Up,
                                position: *other_position,
                            })
                            .is_err()
                });
            }

            for (&id, state) in &packets {
                if state.pressure > 0 {
                    tracing::trace!(
                        id,
                        raw_packet = ?state,
                        final_position = ?state.position,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        rotation,
                        mirror_x,
                        mirror_y,
                        dims = ?dims,
                        "touch packet"
                    );
                }
                if let Some(&pos) = fingers.get(&id) {
                    if state.pressure > 0 {
                        if state.position != pos {
                            ty.send(DeviceEvent::Finger {
                                id,
                                time: seconds(evt.time),
                                status: FingerStatus::Motion,
                                position: state.position,
                            })
                            .unwrap();
                            fingers.insert(id, state.position);
                        }
                    } else {
                        ty.send(DeviceEvent::Finger {
                            id,
                            time: seconds(evt.time),
                            status: FingerStatus::Up,
                            position: state.position,
                        })
                        .unwrap();
                        fingers.remove(&id);
                    }
                } else if state.pressure > 0 {
                    tracing::trace!(
                        id,
                        position = ?state.position,
                        pressure = state.pressure,
                        rotation,
                        mirror_x,
                        mirror_y,
                        dims = ?dims,
                        "finger down"
                    );
                    ty.send(DeviceEvent::Finger {
                        id,
                        time: seconds(evt.time),
                        status: FingerStatus::Down,
                        position: state.position,
                    })
                    .unwrap();
                    fingers.insert(id, state.position);
                }
            }

            if proto != TouchProto::Single {
                packets.clear();
            }
        } else if evt.kind == EV_KEY {
            if SLEEP_COVER.contains(&evt.code) {
                if evt.value == VAL_PRESS {
                    ty.send(DeviceEvent::CoverOn).ok();
                } else if evt.value == VAL_RELEASE {
                    ty.send(DeviceEvent::CoverOff).ok();
                } else if evt.value == VAL_REPEAT {
                    ty.send(DeviceEvent::CoverOn).ok();
                }
            } else if evt.code == KEY_BUTTON_SCHEME {
                if evt.value == VAL_PRESS {
                    button_scheme = ButtonScheme::Inverted;
                } else {
                    button_scheme = ButtonScheme::Natural;
                }
            } else if evt.code == KEY_ROTATE_DISPLAY {
                let next_rotation = evt.value as i8;
                tracing::trace!(
                    from_rotation = rotation,
                    to_rotation = next_rotation,
                    dims_before = ?dims,
                    tc_x = tc.x,
                    tc_y = tc.y,
                    "KEY_ROTATE_DISPLAY received"
                );
                if next_rotation != rotation {
                    let delta = (rotation - next_rotation).abs();
                    let will_swap_axes = delta % 2 == 1;
                    tracing::trace!(
                        delta,
                        will_swap_axes,
                        "KEY_ROTATE_DISPLAY applying rotation"
                    );
                    if will_swap_axes {
                        mem::swap(&mut tc.x, &mut tc.y);
                        if swap_dims_on_rotation {
                            mem::swap(&mut dims.0, &mut dims.1);
                        }
                    }
                    rotation = next_rotation;
                    let should_mirror = compute_mirror_axes(rotation, mirroring_scheme);
                    mirror_x = should_mirror.0;
                    mirror_y = should_mirror.1;
                    tracing::trace!(
                        rotation,
                        mirror_x,
                        mirror_y,
                        dims_after = ?dims,
                        tc_x = tc.x,
                        tc_y = tc.y,
                        axes_swapped = rotation % 2 == swapping_scheme,
                        "rotation applied"
                    );
                }
            } else if evt.code != BTN_TOUCH {
                if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                    ty.send(DeviceEvent::Button {
                        time: seconds(evt.time),
                        code: ButtonCode::from_raw(
                            evt.code,
                            rotation,
                            button_scheme,
                            startup_rotation,
                            dir,
                        ),
                        status: button_status,
                    })
                    .unwrap();
                }
            }
        } else if evt.kind == EV_MSC && evt.code == MSC_RAW {
            if let Some(next_rotation) =
                gsensor_rotation_from_raw(evt.value, gyro_rotation_transform)
            {
                tracing::trace!(
                    raw_value = evt.value,
                    next_rotation,
                    current_rotation = rotation,
                    "gyroscope rotation event"
                );
                ty.send(DeviceEvent::RotateScreen(next_rotation)).ok();
            }
        }
    }
}
