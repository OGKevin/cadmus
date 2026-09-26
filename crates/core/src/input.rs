use crate::framebuffer::Display;
use crate::geom::{LinearDir, Point};
use crate::settings::ButtonScheme;
use anyhow::{Context, Error};
use rustc_hash::FxHashMap;
use std::ffi::CString;
use std::fs::File;
use std::mem::{self, MaybeUninit};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::ptr;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

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

pub fn raw_events(
    paths: Vec<String>,
) -> (UnboundedSender<InputEvent>, UnboundedReceiver<InputEvent>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_reader = tx.clone();
    crate::runtime::current_handle().spawn(async move {
        if let Err(err) = parse_raw_events(&paths, &tx_reader).await {
            tracing::warn!(error = %err, "raw input reader stopped");
        }
    });
    (tx, rx)
}

pub async fn parse_raw_events(
    paths: &[String],
    tx: &UnboundedSender<InputEvent>,
) -> Result<(), Error> {
    let mut watched = Vec::with_capacity(paths.len());
    for path in paths {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .with_context(|| format!("can't open input file {path}"))?;
        let fd = AsyncFd::with_interest(file, Interest::READABLE)
            .with_context(|| format!("can't watch input file {path}"))?;
        watched.push((path.clone(), fd));
    }

    let mut readers = tokio::task::JoinSet::new();
    for (path, fd) in watched {
        let tx = tx.clone();
        readers.spawn(async move {
            if let Err(err) = read_input_events(fd, &tx).await {
                tracing::warn!(path = %path, error = %err, "input device reader stopped");
            }
        });
    }
    while readers.join_next().await.is_some() {}
    Ok(())
}

/// Forwards evdev events from one nonblocking device until it closes or `tx` drops.
///
/// `AsyncFd` only says the fd is readable. A single `read` may still return
/// fewer bytes than `size_of::<InputEvent>()`. Treating
/// that short read as EOF would drop the device for the rest of the session.
/// `pending` / `filled` keep those bytes across `readable()` waits until one
/// full record is available. A zero-length read is the only EOF.
async fn read_input_events(
    fd: AsyncFd<File>,
    tx: &UnboundedSender<InputEvent>,
) -> Result<(), Error> {
    let mut pending = [0_u8; mem::size_of::<InputEvent>()];
    let mut filled = 0usize;
    loop {
        let mut guard = fd.readable().await?;
        loop {
            match guard.try_io(|inner| read_input_event(inner.get_ref(), &mut pending, &mut filled))
            {
                Ok(Ok(event)) => {
                    if tx.send(event).is_err() {
                        return Ok(());
                    }
                }
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Ok(Err(err)) => return Err(err.into()),
                Err(_would_block) => break,
            }
        }
    }
}

/// Reads one `InputEvent`, keeping a short read in `pending` until the next
/// readiness wake. A zero-length read is EOF. `WouldBlock` leaves `filled` as-is.
///
/// # Safety
///
/// `InputEvent` is a C layout with no safe constructor from raw bytes. The
/// copy is sound because [`absorb_record`] returns `Some` only after `pending`
/// holds exactly `size_of::<InputEvent>()` bytes, so every byte is written
/// before `assume_init`. Nothing is read from uninitialised memory.
fn read_input_event(
    file: &File,
    pending: &mut [u8; mem::size_of::<InputEvent>()],
    filled: &mut usize,
) -> std::io::Result<InputEvent> {
    if *filled >= pending.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "evdev buffer overran a single event",
        ));
    }
    let n = read_fd(file.as_raw_fd(), &mut pending[*filled..])?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "evdev closed",
        ));
    }
    let chunk = pending[*filled..*filled + n].to_vec();
    let (record, _) = absorb_record(pending, filled, &chunk);
    let Some(bytes) = record else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "partial evdev event",
        ));
    };
    let mut input_event = MaybeUninit::<InputEvent>::uninit();
    unsafe {
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            input_event.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Ok(input_event.assume_init())
    }
}

/// Appends `chunk` to a fixed-size record buffer. Returns the record once
/// `buf` is full and resets `filled`. A chunk larger than the remaining space
/// fills one record and reports how many bytes were consumed.
fn absorb_record<const N: usize>(
    buf: &mut [u8; N],
    filled: &mut usize,
    chunk: &[u8],
) -> (Option<[u8; N]>, usize) {
    let room = N.saturating_sub(*filled);
    let n = chunk.len().min(room);
    buf[*filled..*filled + n].copy_from_slice(&chunk[..n]);
    *filled += n;
    if *filled == N {
        let done = *buf;
        *filled = 0;
        (Some(done), n)
    } else {
        (None, n)
    }
}

pub fn usb_events() -> UnboundedReceiver<DeviceEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::current_handle().spawn(async move {
        read_usb_events(&tx).await;
    });
    rx
}

/// Per-read size for `/tmp/nickel-hardware-status` (matches typical FIFO chunking).
const USB_STATUS_READ_BYTES: usize = 256;
/// Cap for an incomplete line held across reads: two read buffers, enough for any
/// real nickel status string split across wakes (messages are short ASCII tokens).
const USB_STATUS_PENDING_MAX: usize = USB_STATUS_READ_BYTES * 2;

async fn read_usb_events(tx: &UnboundedSender<DeviceEvent>) {
    let path = CString::new("/tmp/nickel-hardware-status").unwrap();
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_NONBLOCK | libc::O_RDWR) };
    if fd < 0 {
        return;
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let async_fd = match AsyncFd::with_interest(owned, Interest::READABLE) {
        Ok(async_fd) => async_fd,
        Err(err) => {
            tracing::warn!(error = %err, "usb status reader failed");
            return;
        }
    };

    let mut pending = Vec::new();
    loop {
        let mut guard = match async_fd.readable().await {
            Ok(guard) => guard,
            Err(err) => {
                tracing::warn!(error = %err, "usb status wait failed");
                return;
            }
        };
        loop {
            match guard
                .try_io(|inner| read_usb_status(inner.get_ref().as_raw_fd(), &mut pending, tx))
            {
                Ok(Ok(0)) => return,
                Ok(Ok(_)) => {}
                Ok(Err(err)) => {
                    tracing::warn!(error = %err, "usb status read failed");
                    return;
                }
                Err(_would_block) => break,
            }
        }
    }
}

fn read_usb_status(
    fd: RawFd,
    pending: &mut Vec<u8>,
    tx: &UnboundedSender<DeviceEvent>,
) -> std::io::Result<usize> {
    let mut buf = [0u8; USB_STATUS_READ_BYTES];
    let n = read_fd(fd, &mut buf)?;
    if n == 0 {
        return Ok(0);
    }
    let end = buf[..n].iter().position(|byte| *byte == 0).unwrap_or(n);
    feed_usb_status_chunk(pending, &buf[..end], tx);
    Ok(n)
}

/// Appends `chunk` to `pending`, emits complete newline-delimited lines, and
/// retains any trailing fragment for the next read.
fn feed_usb_status_chunk(pending: &mut Vec<u8>, chunk: &[u8], tx: &UnboundedSender<DeviceEvent>) {
    pending.extend_from_slice(chunk);
    if pending.len() > USB_STATUS_PENDING_MAX {
        tracing::warn!(
            pending_len = pending.len(),
            max = USB_STATUS_PENDING_MAX,
            "usb status pending buffer overflow; discarding partial line"
        );
        pending.clear();
        return;
    }

    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let line = pending.drain(..=newline).collect::<Vec<u8>>();
        let line = &line[..line.len() - 1];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        match std::str::from_utf8(line) {
            Ok(message) => send_usb_hardware_message(tx, message),
            Err(_) => tracing::warn!(bytes = ?line, "usb status line is not valid utf-8"),
        }
    }
}

fn read_fd(fd: RawFd, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len() as libc::size_t,
            )
        };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

fn send_usb_hardware_message(tx: &UnboundedSender<DeviceEvent>, message: &str) {
    let event = match message {
        "usb plug add" => DeviceEvent::Plug(PowerSource::Host),
        "usb plug remove" => DeviceEvent::Unplug(PowerSource::Host),
        "usb ac add" => DeviceEvent::Plug(PowerSource::Wall),
        "usb ac remove" => DeviceEvent::Unplug(PowerSource::Wall),
        _ => return,
    };
    tx.send(event).ok();
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
    /// When true, the device-event task swaps logical screen dimensions on 90° rotations.
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
    mut rx: UnboundedReceiver<InputEvent>,
    display: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) -> UnboundedReceiver<DeviceEvent> {
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
    let (ty, ry) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::current_handle().spawn(async move {
        parse_device_events(
            &mut rx,
            &ty,
            Display { dims, rotation },
            button_scheme,
            info,
        )
        .await;
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
pub async fn parse_device_events(
    rx: &mut UnboundedReceiver<InputEvent>,
    ty: &UnboundedSender<DeviceEvent>,
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

    while let Some(evt) = rx.recv().await {
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
                    let time = seconds(evt.time);
                    let code = ButtonCode::from_raw(
                        evt.code,
                        rotation,
                        button_scheme,
                        startup_rotation,
                        dir,
                    );
                    tracing::debug!(
                        code = ?code,
                        status = ?button_status,
                        time,
                        raw_code = evt.code,
                        raw_value = evt.value,
                        "decoded button event"
                    );
                    ty.send(DeviceEvent::Button {
                        time,
                        code,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn feed_usb_status_chunk_reassembles_split_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"usb plug ad", &tx);
        assert!(pending == b"usb plug ad");
        assert!(rx.try_recv().is_err());
        feed_usb_status_chunk(&mut pending, b"d\n", &tx);
        assert!(pending.is_empty());
        assert!(matches!(
            rx.try_recv().expect("plug event"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
    }

    #[test]
    fn feed_usb_status_chunk_handles_multiple_lines_and_crlf() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(
            &mut pending,
            b"usb ac add\r\nusb ac remove\nusb plug remove\n",
            &tx,
        );
        assert!(matches!(
            rx.try_recv().expect("wall plug"),
            DeviceEvent::Plug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("wall unplug"),
            DeviceEvent::Unplug(PowerSource::Wall)
        ));
        assert!(matches!(
            rx.try_recv().expect("host unplug"),
            DeviceEvent::Unplug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn feed_usb_status_chunk_ignores_unknown_lines() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut pending = Vec::new();
        feed_usb_status_chunk(&mut pending, b"noise\nusb plug add\n", &tx);
        assert!(matches!(
            rx.try_recv().expect("host plug"),
            DeviceEvent::Plug(PowerSource::Host)
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn absorb_record_keeps_a_short_read() {
        let mut buf = [0_u8; 4];
        let mut filled = 0usize;
        let (record, n) = absorb_record(&mut buf, &mut filled, &[1, 2]);
        assert!(record.is_none());
        assert_eq!(n, 2);
        assert_eq!(filled, 2);
        let (record, n) = absorb_record(&mut buf, &mut filled, &[3, 4, 5]);
        assert_eq!(record, Some([1, 2, 3, 4]));
        assert_eq!(n, 2);
        assert_eq!(filled, 0);
    }

    #[tokio::test]
    async fn raw_reader_forwards_one_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let c_path = CString::new(path.to_str().expect("utf8 path")).expect("path");
        let _ = std::fs::remove_file(&path);
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let path_str = path.to_str().expect("utf8 path").to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reader_path = path_str.clone();
        let reader = tokio::spawn(async move { parse_raw_events(&[reader_path], &tx).await });

        let writer_path = path.clone();
        let writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let event = InputEvent {
            time: libc::timeval {
                tv_sec: 4,
                tv_usec: 5,
            },
            kind: EV_KEY,
            code: KEY_HOME,
            value: VAL_PRESS,
        };
        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(bytes.as_mut_ptr().cast::<InputEvent>(), event);
        }
        let mut writer = writer;
        writer.write_all(&bytes).expect("write event");
        writer.flush().expect("flush event");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for evdev event")
            .expect("reader closed");
        assert_eq!(got.kind, EV_KEY);
        assert_eq!(got.code, KEY_HOME);
        assert_eq!(got.value, VAL_PRESS);
        assert_eq!(got.time.tv_sec, 4);
        assert_eq!(got.time.tv_usec, 5);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }
}
