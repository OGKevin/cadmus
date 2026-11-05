use std::ptr;

use super::codes::{EV_KEY, KEY_BUTTON_SCHEME, KEY_ROTATE_DISPLAY};
use super::event::{EventSource, InputEvent, TaggedInputEvent};
use super::profile::DeviceProfile;

#[cfg_attr(feature = "tracing", tracing::instrument(fields(rotation = n)))]
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
        source: EventSource::Synthetic,
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(fields(value = v)))]
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
        source: EventSource::Synthetic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::codes::VAL_PRESS;

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
}
