use crate::framebuffer::Display;
use crate::geom::{LinearDir, Point};
use crate::settings::ButtonScheme;
use anyhow::Error;
use nix::ioctl_read;
use nix::ioctl_read_buf;
use rustc_hash::{FxHashMap, FxHashSet};
use std::fs::File;
use std::mem::{self, MaybeUninit};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::ptr;
use std::time::{Duration, Instant};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

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
#[derive(Debug, Clone)]
pub struct InputEvent {
    pub time: libc::timeval,
    pub kind: u16, // type
    pub code: u16,
    pub value: i32,
}

/// How a device's `EV_KEY` events map to semantic actions.
///
/// Each opened evdev device carries a profile so the device-event pipeline
/// routes its `EV_KEY` events through a uniform scancode table ([`key_map`]).
/// Scancodes not in the table are forwarded as [`ButtonCode::Raw`].
///
/// [`key_map`]: DeviceProfile::key_map
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct DeviceProfile {
    key_map: &'static [(u16, KeyAction)],
}

impl DeviceProfile {
    /// Profile for built-in device hardware (touch digitizer, gpio-keys,
    /// power key): maps the standard built-in scancodes to their functions,
    /// including the synthetic control codes injected via
    /// [`display_rotate_event`] / [`button_scheme_event`]. Unknown scancodes
    /// become [`ButtonCode::Raw`].
    const fn built_in() -> Self {
        Self {
            key_map: BUILT_IN_KEY_MAP,
        }
    }

    /// Returns the [`KeyAction`] mapped to `code`, if any.
    fn lookup(self, code: u16) -> Option<KeyAction> {
        self.key_map
            .iter()
            .find_map(|(c, action)| (*c == code).then_some(*action))
    }
}

/// A semantic action mapped to a specific evdev scancode within a
/// [`DeviceProfile`].
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum KeyAction {
    Power,
    Home,
    Light,
    /// Page-turn that respects the button-scheme (rotation-based inversion),
    /// used by built-in page-turn buttons.
    PageTurnScheme(LinearDir),
    Erase,
    Highlight,
    /// Magnetic sleep-cover open/close signal.
    SleepCover,
    /// Synthetic control: rotate the touch coordinate mapping.
    RotateDisplay,
    /// Synthetic control: toggle the button scheme.
    ButtonScheme,
}

impl KeyAction {
    /// Resolves a button-dispatch action to a [`ButtonCode`].
    ///
    /// [`PageTurnScheme`] respects the button-scheme (rotation-based
    /// inversion). Control actions ([`SleepCover`], [`ButtonScheme`],
    /// [`RotateDisplay`]) are handled by their own match arms in
    /// [`parse_device_events`] before this is called, so reaching them here is
    /// a logic error.
    ///
    /// [`PageTurnScheme`]: KeyAction::PageTurnScheme
    /// [`SleepCover`]: KeyAction::SleepCover
    /// [`ButtonScheme`]: KeyAction::ButtonScheme
    /// [`RotateDisplay`]: KeyAction::RotateDisplay
    fn to_button_code(
        self,
        rotation: i8,
        button_scheme: ButtonScheme,
        startup_rotation: i8,
        dir: i8,
    ) -> ButtonCode {
        match self {
            KeyAction::Power => ButtonCode::Power,
            KeyAction::Home => ButtonCode::Home,
            KeyAction::Light => ButtonCode::Light,
            KeyAction::Erase => ButtonCode::Erase,
            KeyAction::Highlight => ButtonCode::Highlight,
            KeyAction::PageTurnScheme(direction) => {
                resolve_button_direction(direction, rotation, button_scheme, startup_rotation, dir)
            }
            KeyAction::SleepCover | KeyAction::ButtonScheme | KeyAction::RotateDisplay => {
                unreachable!("control actions are handled before to_button_code is called")
            }
        }
    }
}

/// Scancode table for [`DeviceProfile::built_in`].
const BUILT_IN_KEY_MAP: &[(u16, KeyAction)] = &[
    (KEY_POWER, KeyAction::Power),
    (KEY_HOME, KeyAction::Home),
    (KEY_LIGHT, KeyAction::Light),
    (KEY_BACKWARD, KeyAction::PageTurnScheme(LinearDir::Backward)),
    (KEY_FORWARD, KeyAction::PageTurnScheme(LinearDir::Forward)),
    (PEN_ERASE, KeyAction::Erase),
    (PEN_HIGHLIGHT, KeyAction::Highlight),
    (SLEEP_COVER[0], KeyAction::SleepCover),
    (SLEEP_COVER[1], KeyAction::SleepCover),
    (KEY_ROTATE_DISPLAY, KeyAction::RotateDisplay),
    (KEY_BUTTON_SCHEME, KeyAction::ButtonScheme),
];

/// An [`InputEvent`] tagged with the [`DeviceProfile`] of the device that
/// produced it, so the device-event pipeline routes its `EV_KEY` events
/// through the device's scancode table.
#[derive(Debug, Clone)]
pub struct TaggedInputEvent {
    pub event: InputEvent,
    pub(crate) profile: DeviceProfile,
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

pub fn display_rotate_event(n: i8) -> TaggedInputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    TaggedInputEvent {
        event: InputEvent {
            time: tp,
            kind: EV_KEY,
            code: KEY_ROTATE_DISPLAY,
            value: n as i32,
        },
        profile: DeviceProfile::built_in(),
    }
}

pub fn button_scheme_event(v: i32) -> TaggedInputEvent {
    let mut tp = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe {
        libc::gettimeofday(&mut tp, ptr::null_mut());
    }
    TaggedInputEvent {
        event: InputEvent {
            time: tp,
            kind: EV_KEY,
            code: KEY_BUTTON_SCHEME,
            value: v,
        },
        profile: DeviceProfile::built_in(),
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
    display: Display,
    proto: TouchProto,
    cancel: CancellationToken,
) -> (
    UnboundedSender<TaggedInputEvent>,
    UnboundedReceiver<TaggedInputEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let tx_reader = tx.clone();
    crate::runtime::current_handle().spawn(async move {
        parse_raw_events(&cancel, display, proto, &tx_reader).await;
    });
    (tx, rx)
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
    tx: &UnboundedSender<TaggedInputEvent>,
) -> Result<(), Error> {
    let mut pending = [0_u8; mem::size_of::<InputEvent>()];
    let mut filled = 0usize;
    loop {
        let mut guard = fd.readable().await?;
        loop {
            match guard.try_io(|inner| read_input_event(inner.get_ref(), &mut pending, &mut filled))
            {
                Ok(Ok(event)) => {
                    // Events from built-in device hardware use the built-in
                    // scancode table; unknown scancodes become `ButtonCode::Raw`.
                    let tagged = TaggedInputEvent {
                        event,
                        profile: DeviceProfile::built_in(),
                    };
                    if tx.send(tagged).is_err() {
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
/// `InputEvent` is a C layout with no safe constructor from raw bytes, so this
/// builds one with `MaybeUninit`. The copy is sound because `pending` only
/// reaches `assume_init` once the read has filled all
/// `size_of::<InputEvent>()` bytes, so every byte is written first.
fn read_input_event(
    file: &File,
    pending: &mut [u8; mem::size_of::<InputEvent>()],
    filled: &mut usize,
) -> std::io::Result<InputEvent> {
    debug_assert!(*filled < pending.len(), "pending record is never left full");
    let n = read_fd(file.as_raw_fd(), &mut pending[*filled..])?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "evdev closed",
        ));
    }
    *filled += n;
    if *filled < pending.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "partial evdev event",
        ));
    }
    let bytes = *pending;
    *filled = 0;
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
    // `OpenOptions` + `custom_flags` rather than a raw `libc::open` +
    // `OwnedFd::from_raw_fd`: the owned `File` closes the fd on drop, so there is
    // no unsafe here and no leak if the async setup below fails.
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/tmp/nickel-hardware-status")
    {
        Ok(file) => file,
        Err(error) => {
            tracing::debug!(error = %error, "usb status fifo unavailable");
            return;
        }
    };
    let owned = OwnedFd::from(file);
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
        // SAFETY: `read` is called with a valid fd from `AsyncFd` and a buffer
        // slice that outlives this call; partial reads and `EINTR` are handled.
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
    mut rx: UnboundedReceiver<TaggedInputEvent>,
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
        skip(rx, ty, screen, button_scheme, info),
        level = tracing::Level::TRACE,
        fields(
            rotation = screen.rotation,
            dims = ?screen.dims,
            proto = ?info.proto,
            mark = info.mark,
            mirroring_scheme = ?info.mirroring_scheme,
            swapping_scheme = info.swapping_scheme,
            startup_rotation = info.startup_rotation,
        )
    )
)]
async fn parse_device_events(
    rx: &mut UnboundedReceiver<TaggedInputEvent>,
    ty: &UnboundedSender<DeviceEvent>,
    screen: Display,
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
    } = info;
    let (_, dir) = mirroring_scheme;
    let mut id = 0;
    let mut last_activity = -60;
    let Display {
        mut dims,
        mut rotation,
    } = screen;
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
        proto = ?proto,
        mark,
        tc_x = tc.x,
        tc_y = tc.y,
        axes_swapped,
        "parse_device_events started"
    );

    let mut button_scheme = button_scheme;

    while let Some(tagged) = rx.recv().await {
        let evt = tagged.event;
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
                    tracing::debug!(
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
                    tracing::debug!(
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
            if evt.code == BTN_TOUCH {
                // Touch-contact signal from a touch/pen device; not a button.
            } else {
                match tagged.profile.lookup(evt.code) {
                    Some(KeyAction::SleepCover) => {
                        if evt.value == VAL_PRESS || evt.value == VAL_REPEAT {
                            ty.send(DeviceEvent::CoverOn).ok();
                        } else if evt.value == VAL_RELEASE {
                            ty.send(DeviceEvent::CoverOff).ok();
                        }
                    }
                    Some(KeyAction::ButtonScheme) => {
                        button_scheme = if evt.value == VAL_PRESS {
                            ButtonScheme::Inverted
                        } else {
                            ButtonScheme::Natural
                        };
                    }
                    Some(KeyAction::RotateDisplay) => {
                        let next_rotation = evt.value as i8;
                        tracing::debug!(
                            from_rotation = rotation,
                            to_rotation = next_rotation,
                            dims_before = ?dims,
                            mirror_x_before = mirror_x,
                            mirror_y_before = mirror_y,
                            tc_x = tc.x,
                            tc_y = tc.y,
                            "KEY_ROTATE_DISPLAY received"
                        );
                        if next_rotation != rotation {
                            let delta = (rotation - next_rotation).abs();
                            let will_swap_axes = delta % 2 == 1;
                            if will_swap_axes {
                                mem::swap(&mut tc.x, &mut tc.y);
                                mem::swap(&mut dims.0, &mut dims.1);
                            }
                            rotation = next_rotation;
                            let should_mirror = compute_mirror_axes(rotation, mirroring_scheme);
                            mirror_x = should_mirror.0;
                            mirror_y = should_mirror.1;
                            tracing::debug!(
                                delta,
                                will_swap_axes,
                                rotation,
                                mirror_x,
                                mirror_y,
                                dims_after = ?dims,
                                tc_x = tc.x,
                                tc_y = tc.y,
                                axes_swapped = rotation % 2 == swapping_scheme,
                                "rotation applied to input mapping"
                            );
                        } else {
                            tracing::debug!(
                                rotation,
                                "KEY_ROTATE_DISPLAY matches current input rotation; no remap"
                            );
                        }
                    }
                    Some(action) => {
                        if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                            let time = seconds(evt.time);
                            let code = action.to_button_code(
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
                            .ok();
                        }
                    }
                    None => {
                        if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                            let time = seconds(evt.time);
                            let code = ButtonCode::Raw(evt.code);
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
                            .ok();
                        }
                    }
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

// Ioctl wrappers for evdev device introspection.
//
// `EVIOCGID` returns the bustype/vendor/product/version tuple used to
// deduplicate devices that the kernel exposes under multiple event nodes.
// `EVIOCGEV` returns the bitmap of event types supported by a device, used
// to filter for input devices (those that emit `EV_KEY` or `EV_ABS`).
ioctl_read!(eviocgid, b'E', 0x02, InputId);

#[repr(C)]
#[derive(Debug, Clone)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

ioctl_read_buf!(eviocgbit_ev, b'E', 0x20, u8);

// `EVIOCGNAME` — returns the kernel device name string, included in the
// deduplication identity so distinct devices that share an `EVIOCGID` tuple
// (e.g. `gpio-keys` and `bd71828-pwrkey` on current Kobo hardware) are not
// collapsed into one.
ioctl_read_buf!(eviocgname, b'E', 0x06, u8);

/// Kernel-reported features of an evdev device, probed via `EVIOCGID`,
/// `EVIOCGNAME`, and `EVIOCGEV`. Used to decide whether a device should be
/// opened (it must emit `EV_KEY` or `EV_ABS`) and to deduplicate devices
/// exposed under multiple event nodes.
struct DeviceFeatures {
    name: Option<String>,
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    emits_key: bool,
    emits_abs: bool,
}

impl DeviceFeatures {
    /// Returns a stable identity string for deduplication, based on the
    /// device name and the bustype/vendor/product/version tuple. Returns
    /// `None` when the device reports no identity (all zeros and no name),
    /// so callers fall back to path-based deduplication.
    ///
    /// The device name is included because distinct built-in devices can
    /// share an `EVIOCGID` tuple: `gpio-keys` and `bd71828-pwrkey` both
    /// report `bus 0x19, vendor 0x1, product 0x1, version 0x100` on current
    /// Kobo hardware. Without the name, the deduplicator would treat the
    /// power key as a duplicate of the page-turn buttons and drop one of
    /// them.
    fn identity(&self) -> Option<String> {
        let name = self.name.as_deref().unwrap_or("");
        if name.is_empty() && self.bustype == 0 && self.vendor == 0 && self.product == 0 {
            return None;
        }
        Some(format!(
            "id:{}:{:04x}:{:04x}:{:04x}:{:04x}",
            name, self.bustype, self.vendor, self.product, self.version
        ))
    }
}

/// Probes the device at `path` and returns its [`DeviceFeatures`].
///
/// Opens the device once and issues all introspection ioctls on the resulting
/// file descriptor. Returns `None` only if the device cannot be opened.
fn probe_device(path: &str) -> Option<DeviceFeatures> {
    let file = File::open(path).ok()?;
    let fd = file.as_raw_fd();
    let mut id = InputId {
        bustype: 0,
        vendor: 0,
        product: 0,
        version: 0,
    };
    let _ = unsafe { eviocgid(fd, &mut id) };

    let mut name_buf = [0u8; 256];
    let name = unsafe { eviocgname(fd, &mut name_buf) }.ok().and_then(|n| {
        std::str::from_utf8(&name_buf[..n as usize])
            .ok()
            .map(|s| s.trim_end_matches('\0').to_string())
    });

    let (emits_key, emits_abs) = {
        let mut evbits = [0u8; 4];
        if unsafe { eviocgbit_ev(fd, &mut evbits) }.is_err() {
            (false, false)
        } else {
            // EV_KEY is event type 1; EV_ABS is event type 3.
            ((evbits[0] & (1 << 1)) != 0, (evbits[0] & (1 << 3)) != 0)
        }
    };

    Some(DeviceFeatures {
        name,
        bustype: id.bustype,
        vendor: id.vendor,
        product: id.product,
        version: id.version,
        emits_key,
        emits_abs,
    })
}

/// Scans `/dev/input/event*` for input devices not already tracked.
///
/// Returns each new device's path alongside its probed [`DeviceFeatures`] so the
/// caller can decide whether to trace its geometry without re-probing. Excludes
/// paths in `tracked_paths` (already opened persistent + removable devices) and
/// `blacklisted_paths` (known duplicates). Deduplicates by device identity so a
/// device exposed under multiple event nodes is only opened once; duplicates are
/// added to `blacklisted_paths` to avoid re-querying them on every scan.
fn scan_input_devices(
    tracked_paths: &[String],
    known_identities: &mut FxHashSet<String>,
    blacklisted_paths: &mut FxHashSet<String>,
) -> Vec<(String, DeviceFeatures)> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir("/dev/input") {
        Ok(e) => e,
        Err(_) => return found,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(path_str) = path.to_str() else {
            continue;
        };
        if !path_str.starts_with("/dev/input/event") {
            continue;
        }
        if tracked_paths.iter().any(|p| p == path_str) || blacklisted_paths.contains(path_str) {
            continue;
        }
        let Some(features) = probe_device(path_str) else {
            continue;
        };
        if !features.emits_key && !features.emits_abs {
            continue;
        }
        match features.identity() {
            Some(identity) => {
                if known_identities.insert(identity.clone()) {
                    tracing::debug!(device = path_str, identity, "scan: new device");
                    found.push((path_str.to_string(), features));
                } else {
                    tracing::debug!(
                        device = path_str,
                        identity,
                        "scan: blacklisting duplicate device"
                    );
                    blacklisted_paths.insert(path_str.to_string());
                }
            }
            None => {
                tracing::debug!(device = path_str, "scan: device without identity");
                found.push((path_str.to_string(), features));
            }
        }
    }

    found
}

/// Reads raw evdev events from all input devices and forwards them as
/// [`TaggedInputEvent`]s on `tx`.
///
/// Every device under `/dev/input/event*` is probed at startup and opened as a
/// **persistent** device. Built-in nodes (`/dev/input/eventN`) outlive driver
/// hiccups, so a reader that exits from a driver error is re-opened on the next
/// scan rather than dropped for the rest of the session. A periodic rescan
/// discovers **removable** devices hotplugged after startup, and when a reader
/// stops the device's identity is released so a replug is re-detected instead of
/// being treated as a duplicate.
async fn parse_raw_events(
    cancel: &CancellationToken,
    display: Display,
    proto: TouchProto,
    tx: &UnboundedSender<TaggedInputEvent>,
) {
    /// How often `/dev/input/event*` is rescanned for hotplugged devices.
    const SCAN_INTERVAL: Duration = Duration::from_secs(5);

    let mut device = OpenInputDevices {
        watched: Vec::new(),
        readers: tokio::task::JoinSet::new(),
        known_identities: FxHashSet::default(),
        blacklisted_paths: FxHashSet::default(),
        proto,
        display,
        tx,
    };

    device.open(&[], false);

    let mut last_scan = Instant::now();
    loop {
        let next_scan = SCAN_INTERVAL.saturating_sub(last_scan.elapsed());
        let readers_busy = !device.readers.is_empty();
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(next_scan) => {}
            Some(joined) = device.readers.join_next(), if readers_busy => {
                device.reap(joined);
            }
        }

        if last_scan.elapsed() >= SCAN_INTERVAL {
            last_scan = Instant::now();
            device.open(&device.tracked(), true);
        }
    }
}

/// Owns the evdev devices the input reader currently watches.
///
/// One reader task per open device forwards events on `tx`; when a task ends it
/// returns the device's path and dedup identity so [`Self::reap`] can release
/// them, letting a later scan re-open a persistent node or re-detect a replugged
/// removable one.
struct OpenInputDevices<'a> {
    watched: Vec<(String, Option<String>)>,
    readers: tokio::task::JoinSet<(String, Option<String>)>,
    known_identities: FxHashSet<String>,
    blacklisted_paths: FxHashSet<String>,
    proto: TouchProto,
    display: Display,
    tx: &'a UnboundedSender<TaggedInputEvent>,
}

impl OpenInputDevices<'_> {
    /// Paths currently watched, passed to the scan as already tracked.
    fn tracked(&self) -> Vec<String> {
        self.watched.iter().map(|(path, _)| path.clone()).collect()
    }

    /// Scans for devices not already tracked and opens a reader for each.
    ///
    /// `hotplugged` only distinguishes the log line between the startup set and
    /// devices that appeared later.
    fn open(&mut self, tracked: &[String], hotplugged: bool) {
        for (path, features) in scan_input_devices(
            tracked,
            &mut self.known_identities,
            &mut self.blacklisted_paths,
        ) {
            let file = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => file,
                Err(error) => {
                    tracing::warn!(
                        device = %path,
                        error = %error,
                        "failed to open input device; skipping"
                    );
                    continue;
                }
            };
            if features.emits_abs {
                trace_touch_device_geometry(&path, self.proto, self.display);
            }
            let fd = match AsyncFd::with_interest(file, Interest::READABLE) {
                Ok(fd) => fd,
                Err(error) => {
                    tracing::warn!(
                        device = %path,
                        error = %error,
                        "failed to watch input device; skipping"
                    );
                    continue;
                }
            };
            tracing::info!(
                device = %path,
                emits_key = features.emits_key,
                emits_abs = features.emits_abs,
                hotplugged,
                "opened input device"
            );
            let identity = features.identity();
            let tx = self.tx.clone();
            let path_for_reader = path.clone();
            let identity_for_reader = identity.clone();
            self.readers.spawn(async move {
                let outcome = read_input_events(fd, &tx).await;
                tracing::debug!(
                    device = %path_for_reader,
                    error = ?outcome.as_ref().err(),
                    "input device reader exited"
                );
                (path_for_reader, identity_for_reader)
            });
            self.watched.push((path, identity));
        }
    }

    /// Releases the device whose reader just ended.
    fn reap(&mut self, joined: Result<(String, Option<String>), tokio::task::JoinError>) {
        if let Ok((path, identity)) = joined {
            if let Some(pos) = self.watched.iter().position(|(p, _)| p == &path) {
                self.watched.remove(pos);
            }
            if let Some(identity) = identity.as_ref() {
                self.known_identities.remove(identity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
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

    /// Regression test for [GitHub issue #1089](https://github.com/OGKevin/cadmus/issues/1089):
    /// a quarter-turn rotation must swap the cached input dimensions so
    /// mirrored touch positions use the rotated screen bounds. The regression
    /// was introduced in commit `9b91d69a4`.
    #[tokio::test]
    async fn touch_mapping_swaps_dims_when_rotating_to_landscape() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (2, 1),
            swapping_scheme: 1,
            startup_rotation: 1,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        let screen = Display {
            dims: (1264, 1680),
            rotation: 1,
        };
        tokio::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(&mut rx, &ty, screen, ButtonScheme::Natural, info).await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
        };
        for evt in [
            tagged(EV_KEY, KEY_ROTATE_DISPLAY, 2),
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(EV_ABS, ABS_MT_POSITION_X, 59),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 1178),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }

        let finger = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    position,
                    ..
                }) = ry.recv().await
                {
                    return position;
                }
            }
        })
        .await
        .expect("timed out waiting for touch position");

        assert_eq!(finger, Point { x: 1620, y: 85 });
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
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = tokio::spawn(async move { read_input_events(fd, &tx).await });

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
        assert_eq!(got.profile, DeviceProfile::built_in());
        assert_eq!(got.event.kind, EV_KEY);
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.value, VAL_PRESS);
        assert_eq!(got.event.time.tv_sec, 4);
        assert_eq!(got.event.time.tv_usec, 5);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }

    /// A record split across two reads must be held until complete, and must
    /// not be mistaken for EOF.
    #[tokio::test]
    async fn raw_reader_holds_a_split_evdev_event() {
        let path = std::env::temp_dir().join(format!(
            "cadmus-evdev-split-{}-{}",
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
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path_str)
            .expect("open fifo");
        let fd = AsyncFd::with_interest(file, Interest::READABLE).expect("watch fifo");
        let reader = tokio::spawn(async move { read_input_events(fd, &tx).await });

        let writer_path = path.clone();
        let mut writer = tokio::task::spawn_blocking(move || {
            std::fs::OpenOptions::new().write(true).open(writer_path)
        })
        .await
        .expect("writer task")
        .expect("open fifo");

        let mut bytes = vec![0u8; mem::size_of::<InputEvent>()];
        unsafe {
            std::ptr::write_unaligned(
                bytes.as_mut_ptr().cast::<InputEvent>(),
                InputEvent {
                    time: libc::timeval {
                        tv_sec: 9,
                        tv_usec: 1,
                    },
                    kind: EV_KEY,
                    code: KEY_HOME,
                    value: VAL_PRESS,
                },
            );
        }
        let split = mem::size_of::<InputEvent>() / 2;

        writer.write_all(&bytes[..split]).expect("write first half");
        writer.flush().expect("flush first half");
        assert!(
            rx.try_recv().is_err(),
            "a half record must not be forwarded"
        );

        writer
            .write_all(&bytes[split..])
            .expect("write second half");
        writer.flush().expect("flush second half");

        let got = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for the joined evdev event")
            .expect("reader closed");
        assert_eq!(got.event.code, KEY_HOME);
        assert_eq!(got.event.time.tv_sec, 9);

        drop(writer);
        reader.abort();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn display_rotate_event_uses_built_in_profile() {
        let event = display_rotate_event(1);
        assert_eq!(event.profile, DeviceProfile::built_in());
        assert_eq!(event.event.kind, EV_KEY);
        assert_eq!(event.event.code, KEY_ROTATE_DISPLAY);
        assert_eq!(event.event.value, 1);
    }

    #[test]
    fn button_scheme_event_uses_built_in_profile() {
        let event = button_scheme_event(VAL_PRESS);
        assert_eq!(event.profile, DeviceProfile::built_in());
        assert_eq!(event.event.code, KEY_BUTTON_SCHEME);
        assert_eq!(event.event.value, VAL_PRESS);
    }

    #[test]
    fn built_in_profile_maps_known_scancodes() {
        let profile = DeviceProfile::built_in();
        assert_eq!(profile.lookup(KEY_POWER), Some(KeyAction::Power));
        assert_eq!(profile.lookup(KEY_HOME), Some(KeyAction::Home));
        assert_eq!(profile.lookup(KEY_LIGHT), Some(KeyAction::Light));
        assert_eq!(
            profile.lookup(KEY_BACKWARD),
            Some(KeyAction::PageTurnScheme(LinearDir::Backward))
        );
        assert_eq!(
            profile.lookup(KEY_FORWARD),
            Some(KeyAction::PageTurnScheme(LinearDir::Forward))
        );
        assert_eq!(profile.lookup(PEN_ERASE), Some(KeyAction::Erase));
        assert_eq!(profile.lookup(PEN_HIGHLIGHT), Some(KeyAction::Highlight));
        assert_eq!(profile.lookup(SLEEP_COVER[0]), Some(KeyAction::SleepCover));
        assert_eq!(profile.lookup(SLEEP_COVER[1]), Some(KeyAction::SleepCover));
        assert_eq!(
            profile.lookup(KEY_ROTATE_DISPLAY),
            Some(KeyAction::RotateDisplay)
        );
        assert_eq!(
            profile.lookup(KEY_BUTTON_SCHEME),
            Some(KeyAction::ButtonScheme)
        );
    }

    #[test]
    fn built_in_profile_returns_none_for_unknown_scancodes() {
        assert_eq!(DeviceProfile::built_in().lookup(999), None);
    }

    #[test]
    fn key_action_to_button_code_maps_simple_actions() {
        // rotation=0, Natural scheme, startup_rotation=0, dir=1.
        assert_eq!(
            KeyAction::Power.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Power
        );
        assert_eq!(
            KeyAction::Home.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Home
        );
        assert_eq!(
            KeyAction::Light.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Light
        );
        assert_eq!(
            KeyAction::Erase.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Erase
        );
        assert_eq!(
            KeyAction::Highlight.to_button_code(0, ButtonScheme::Natural, 0, 1),
            ButtonCode::Highlight
        );
    }

    #[test]
    fn key_action_page_turn_scheme_respects_button_scheme() {
        // At rotation 0 with Natural, Forward stays Forward.
        assert_eq!(
            KeyAction::PageTurnScheme(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Natural,
                0,
                1
            ),
            ButtonCode::Forward
        );
        // With Inverted, Forward flips to Backward.
        assert_eq!(
            KeyAction::PageTurnScheme(LinearDir::Forward).to_button_code(
                0,
                ButtonScheme::Inverted,
                0,
                1
            ),
            ButtonCode::Backward
        );
    }
}
