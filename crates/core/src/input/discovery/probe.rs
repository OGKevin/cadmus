use std::mem::{self, MaybeUninit};
use std::os::fd::AsRawFd;

use nix::{ioctl_read, ioctl_read_buf};

use crate::framebuffer::Display;
use crate::input::codes::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_X, ABS_Y, EV_ABS};
use crate::input::touch::TouchProto;

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

/// Probes one axis of the device at `path` via `EVIOCGABS`.
///
/// The ioctl writes into an uninitialized [`InputAbsInfo`] and returns `0`
/// on success; on failure (`-1`) the buffer is left untouched, so the value
/// is only read back after a non-negative return and a failed probe yields
/// `None` instead of partial data.
#[cfg_attr(feature = "tracing", tracing::instrument)]
async fn evdev_abs_info(path: &str, axis: u16) -> Option<InputAbsInfo> {
    let file = tokio::fs::File::open(path).await.ok()?;
    let fd = file.as_raw_fd();
    let request = evdev_abs_ioctl(axis);
    let ret = crate::runtime::spawn_blocking(move || unsafe {
        let mut absinfo = MaybeUninit::<InputAbsInfo>::uninit();
        let ret = libc::ioctl(fd, request as libc::c_ulong, absinfo.as_mut_ptr());
        (ret, absinfo)
    })
    .await
    .ok()?;
    let (ret, absinfo) = ret;
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
/// `min..max` reported by `EVIOCGABS` for one axis of one device.
type AbsRange = (i32, i32);

/// Per-axis `EVIOCGABS` ranges of one device: `x` then `y`.
pub(super) type TouchAxes = (Option<AbsRange>, Option<AbsRange>);
#[cfg_attr(feature = "tracing", tracing::instrument)]
pub(super) async fn trace_touch_device_geometry(
    path: &str,
    proto: TouchProto,
    screen: Display,
) -> TouchAxes {
    let Display { dims, rotation } = screen;
    let (x_axis, y_axis) = match proto {
        TouchProto::Single => (ABS_X, ABS_Y),
        TouchProto::MultiA | TouchProto::MultiB | TouchProto::MultiC => {
            (ABS_MT_POSITION_X, ABS_MT_POSITION_Y)
        }
    };
    let abs_x = evdev_abs_info(path, x_axis).await;
    let abs_y = evdev_abs_info(path, y_axis).await;
    tracing::trace!(
        path,
        proto = ?proto,
        screen_rotation = rotation,
        screen_dims = ?dims,
        abs_x = ?abs_x,
        abs_y = ?abs_y,
        "evdev vs display geometry"
    );
    let range = |info: Option<InputAbsInfo>| info.map(|info| (info.minimum, info.maximum));
    (range(abs_x), range(abs_y))
}
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
ioctl_read_buf!(eviocgbit_abs, b'E', 0x20 + EV_ABS, u8);

ioctl_read_buf!(eviocgname, b'E', 0x06, u8);
/// Kernel-reported features of an evdev device, probed via `EVIOCGID`,
/// `EVIOCGNAME`, and `EVIOCGEV`. Used to classify each device into a
/// [`DeviceProfile`] and to deduplicate devices exposed under multiple event
/// nodes.
pub(super) struct DeviceFeatures {
    pub(super) name: Option<String>,
    pub(super) bustype: u16,
    pub(super) vendor: u16,
    pub(super) product: u16,
    pub(super) version: u16,
    pub(super) emits_key: bool,
    pub(super) emits_abs: bool,
    /// The device emits `EV_ABS` events with position axes (`ABS_X`/`ABS_Y` or
    /// `ABS_MT_POSITION_X`/`ABS_MT_POSITION_Y`), i.e. it is a touch or pen
    /// digitizer. Keyboards whose HID descriptor declares a stray axis (e.g.
    /// `ABS_MISC`) do not qualify.
    pub(super) has_position_axes: bool,
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    pub(super) fn identity(&self) -> Option<String> {
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

/// Returns `true` if the ABS-axes bitmap (`EVIOCGBIT(EV_ABS, ...)`) declares
/// support for `axis`.
fn has_abs_axis(absbits: &[u8; 8], axis: u16) -> bool {
    (absbits[axis as usize / 8] & (1 << (axis % 8))) != 0
}
/// Probes the device at `path` and returns its [`DeviceFeatures`].
///
/// Opens the device once and issues all introspection ioctls on the resulting
/// file descriptor. Returns `None` only if the device cannot be opened.
/// # Safety of ioctls
///
/// The ioctls write into pre-zeroed buffers and their results are checked, so
/// partial failures leave defaults in place.
///
/// The ioctls are issued inside `spawn_blocking`: they have no async
/// equivalent and must not block the Tokio worker pool.
#[cfg_attr(feature = "tracing", tracing::instrument)]
pub(super) async fn probe_device(path: &str) -> Option<DeviceFeatures> {
    let file = tokio::fs::File::open(path).await.ok()?;
    crate::runtime::spawn_blocking(move || {
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
                ((evbits[0] & (1 << 1)) != 0, (evbits[0] & (1 << 3)) != 0)
            }
        };

        let has_position_axes = emits_abs && {
            let mut absbits = [0u8; 8];
            unsafe { eviocgbit_abs(fd, &mut absbits) }.is_ok()
                && (has_abs_axis(&absbits, ABS_X) || has_abs_axis(&absbits, ABS_MT_POSITION_X))
        };

        DeviceFeatures {
            name,
            bustype: id.bustype,
            vendor: id.vendor,
            product: id.product,
            version: id.version,
            emits_key,
            emits_abs,
            has_position_axes,
        }
    })
    .await
    .ok()
}

#[cfg(test)]
mod tests {
    use super::super::classify::BUS_HOST;
    use super::*;

    /// `has_abs_axis` must decode the ABS bitmap exactly as the kernel
    /// reports it: bit 40 (ABS_MISC) in byte 5 for the uhid keyboard, and
    /// position axes in byte 0 (single-touch) or byte 6 (multi-touch) for
    /// digitizers.
    #[test]
    fn has_abs_axis_decodes_real_device_bitmaps() {
        let targus = [0u8, 0, 0, 0, 0, 0x01, 0, 0];
        assert!(!has_abs_axis(&targus, ABS_X));
        assert!(!has_abs_axis(&targus, ABS_MT_POSITION_X));
        assert!(has_abs_axis(&targus, 40));

        let single_touch = [0x03u8, 0, 0, 0, 0, 0, 0, 0];
        assert!(has_abs_axis(&single_touch, ABS_X));
        assert!(has_abs_axis(&single_touch, ABS_Y));

        let multi_touch = [0u8, 0, 0, 0, 0, 0, 0x60, 0];
        assert!(has_abs_axis(&multi_touch, ABS_MT_POSITION_X));
        assert!(has_abs_axis(&multi_touch, 54));
    }
    /// Reproduces the bug where `bd71828-pwrkey` and `gpio-keys` share the
    /// same `EVIOCGID` tuple on current Kobo hardware: the deduplicator
    /// treated one as a duplicate of the other and dropped the power key.
    /// With the name included in the identity, both devices are kept.
    #[test]
    fn identity_distinguishes_pwrkey_from_gpio_keys() {
        let shared_id = (BUS_HOST, 0x1, 0x1, 0x100);
        let pwrkey = DeviceFeatures {
            name: Some("bd71828-pwrkey".to_string()),
            bustype: shared_id.0,
            vendor: shared_id.1,
            product: shared_id.2,
            version: shared_id.3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let gpio_keys = DeviceFeatures {
            name: Some("gpio-keys".to_string()),
            bustype: shared_id.0,
            vendor: shared_id.1,
            product: shared_id.2,
            version: shared_id.3,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        assert_ne!(pwrkey.identity(), gpio_keys.identity());
    }
    #[test]
    fn identity_returns_none_for_anonymous_zero_id() {
        let f = DeviceFeatures {
            name: None,
            bustype: 0,
            vendor: 0,
            product: 0,
            version: 0,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        assert_eq!(f.identity(), None);
    }
    #[test]
    fn identity_includes_name_in_output() {
        let f = DeviceFeatures {
            name: Some("gpio-keys".to_string()),
            bustype: BUS_HOST,
            vendor: 0x1,
            product: 0x1,
            version: 0x100,
            emits_key: true,
            emits_abs: false,
            has_position_axes: false,
        };
        let id = f.identity().expect("named device has an identity");
        assert!(id.contains("gpio-keys"), "identity must include name: {id}");
    }
}
