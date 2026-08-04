//! Autosleep target mode for soft suspend.

use crate::fl;
use crate::i18n::I18nDisplay;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Kernel autosleep target selected by the user.
#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutosleepMode {
    /// Autosleep disabled (`off`).
    #[default]
    Off,
    /// Freeze userspace / light sleep.
    Freeze,
    /// Suspend to RAM (`mem`).
    Mem,
}

impl AutosleepMode {
    /// Returns the sysfs value written to `/sys/power/autosleep`.
    pub fn as_sysfs(self) -> &'static str {
        match self {
            AutosleepMode::Off => "off",
            AutosleepMode::Freeze => "freeze",
            AutosleepMode::Mem => "mem",
        }
    }

    /// Parses a token from `/sys/power/state` (not including `off`).
    pub fn from_state_token(token: &str) -> Option<Self> {
        match token {
            "freeze" => Some(AutosleepMode::Freeze),
            "mem" => Some(AutosleepMode::Mem),
            _ => None,
        }
    }

    /// Returns whether this mode arms autosleep.
    pub fn is_armed(self) -> bool {
        !matches!(self, AutosleepMode::Off)
    }
}

impl fmt::Display for AutosleepMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_sysfs())
    }
}

impl I18nDisplay for AutosleepMode {
    fn to_i18n_string(&self) -> String {
        match self {
            AutosleepMode::Off => fl!("settings-power-autosleep-mode-off"),
            AutosleepMode::Freeze => fl!("settings-power-autosleep-mode-freeze"),
            AutosleepMode::Mem => fl!("settings-power-autosleep-mode-mem"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_sysfs_matches_kernel_tokens() {
        assert_eq!(AutosleepMode::Off.as_sysfs(), "off");
        assert_eq!(AutosleepMode::Freeze.as_sysfs(), "freeze");
        assert_eq!(AutosleepMode::Mem.as_sysfs(), "mem");
    }

    #[test]
    fn from_state_token_parses_known_targets() {
        assert_eq!(
            AutosleepMode::from_state_token("freeze"),
            Some(AutosleepMode::Freeze)
        );
        assert_eq!(
            AutosleepMode::from_state_token("mem"),
            Some(AutosleepMode::Mem)
        );
    }

    #[test]
    fn from_state_token_rejects_off_and_unknown() {
        assert_eq!(AutosleepMode::from_state_token("off"), None);
        assert_eq!(AutosleepMode::from_state_token("disk"), None);
        assert_eq!(AutosleepMode::from_state_token(""), None);
    }

    #[test]
    fn is_armed_only_for_sleep_targets() {
        assert!(!AutosleepMode::Off.is_armed());
        assert!(AutosleepMode::Freeze.is_armed());
        assert!(AutosleepMode::Mem.is_armed());
    }

    #[test]
    fn display_uses_sysfs_token() {
        assert_eq!(AutosleepMode::Off.to_string(), "off");
        assert_eq!(AutosleepMode::Freeze.to_string(), "freeze");
        assert_eq!(AutosleepMode::Mem.to_string(), "mem");
    }

    #[test]
    fn default_is_off() {
        assert_eq!(AutosleepMode::default(), AutosleepMode::Off);
    }
}
