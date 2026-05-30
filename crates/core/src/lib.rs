#[macro_use]
pub mod geom;

pub mod assets;
pub mod battery;
pub mod color;
pub mod context;
pub mod crypto;
pub mod db;
pub mod device;
cfg_select! {
    feature = "bench" => { pub mod dictionary; }
    _ => { mod dictionary; }
}
pub mod document;
pub mod font;
pub mod framebuffer;
pub mod frontlight;
pub mod gesture;
pub mod github;
pub mod helpers;
pub mod http;
pub mod i18n;
pub mod input;
pub mod library;
pub mod lightsensor;
pub mod logging;
pub mod metadata;
pub mod ota;
pub mod rtc;

pub use rtc::{AlarmManager, AlarmType};
pub mod settings;
pub mod task;
#[cfg(any(feature = "profiling", feature = "tracing"))]
pub mod telemetry;
mod unit;
pub mod version;
pub mod view;

pub use anyhow;
pub use chrono;
pub use ctor;
pub use fxhash;
pub use globset;
pub use png;
pub use rand_core;
pub use rand_xoshiro;
pub use serde;
pub use serde_json;
pub use walkdir;
