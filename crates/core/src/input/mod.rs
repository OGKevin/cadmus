mod codes;
mod decoder;
#[cfg(target_os = "linux")]
mod discovery;
mod event;
mod keyboard;
mod profile;
#[cfg(target_os = "linux")]
mod reader;
mod synthetic;
mod sys;
mod touch;
mod usb;

pub use codes::{EV_KEY, KEY_ROTATE_DISPLAY, VAL_PRESS, VAL_RELEASE};
pub use decoder::{DeviceInputInfo, GyroRotationTransform, device_events};
pub use event::{
    ButtonCode, ButtonStatus, DeviceEvent, FingerStatus, InputEvent, PowerSource, TaggedInputEvent,
};
pub use keyboard::{KeyEvent, KeyState, Modifier, Modifiers};
#[cfg(target_os = "linux")]
pub use reader::raw_events;
pub use synthetic::{button_scheme_event, display_rotate_event};
pub use touch::TouchProto;
pub use usb::usb_events;
