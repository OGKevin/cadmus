//! Named SoftSuspend lease holders.
//!
//! These names are for [`Kind::SoftSuspend`](super::super::Kind::SoftSuspend)
//! only. Full holders use free-form names such as `"ota"`. The nested wake
//! lock taken while Full is active is the string `"full-inhibit"`, which is
//! **not** a variant here.

use crate::lease::LeaseName;
use std::fmt;

/// SoftSuspend lease name written into the holder tracker / logs.
///
/// The kernel still sees one `cadmus` wake lock; these names only distinguish
/// holders in logs and [`SoftSuspendBackend::holders`](crate::device::soft_suspend::SoftSuspendBackend::holders).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoftSuspendName {
    /// Gesture / touch / USB input enqueued on the hub.
    Input,
    /// RTC alarm IRQ delivered on the hub.
    Rtc,
    /// WiFi radio soft-suspend pin while online or AlwaysOn.
    Wifi,
    /// Main loop overlap while handling a hub event.
    MainLoop,
    /// Held across process shutdown work.
    Shutdown,
    /// Explicit deep-idle suspend cycle.
    DeepIdle,
    /// Thumbnail extraction background task.
    Thumbnail,
    /// Library import background task.
    LibraryImport,
    /// Dictionary index background task.
    DictionaryIndex,
    /// Unit / integration tests.
    Test,
}

impl SoftSuspendName {
    /// Returns the stable string written to holders and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Rtc => "rtc",
            Self::Wifi => "wifi",
            Self::MainLoop => "main-loop",
            Self::Shutdown => "shutdown",
            Self::DeepIdle => "deep-idle",
            Self::Thumbnail => "thumbnail",
            Self::LibraryImport => "library-import",
            Self::DictionaryIndex => "dictionary-index",
            Self::Test => "test",
        }
    }
}

impl From<SoftSuspendName> for LeaseName {
    fn from(value: SoftSuspendName) -> Self {
        LeaseName::new(value.as_str())
    }
}

impl fmt::Display for SoftSuspendName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
