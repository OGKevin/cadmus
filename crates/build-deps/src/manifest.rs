//! `Cargo.toml` parsing helpers shared between `build.rs` and `xtask`.

use crate::cargo_features::{
    any_named_feature_enabled, collect_enabled_feature_names, format_enabled_features,
};
use anyhow::{Result, bail};

pub const DEVICE_LIST_START: &str = "# device-list-start";
pub const DEVICE_LIST_END: &str = "# device-list-end";

/// Parses feature names between the device-list markers in a `[features]` table.
///
/// # Errors
///
/// Returns an error when no device features are found between the markers.
pub fn parse_device_features(manifest_content: &str) -> Result<Vec<String>> {
    let mut in_features = false;
    let mut in_device_list = false;
    let mut device_features = Vec::new();

    for line in manifest_content.lines() {
        let trimmed = line.trim();
        if trimmed == "[features]" {
            in_features = true;
            continue;
        }
        if in_features && trimmed.starts_with('[') && trimmed.ends_with(']') {
            break;
        }
        if !in_features {
            continue;
        }
        if trimmed == DEVICE_LIST_START {
            in_device_list = true;
            continue;
        }
        if trimmed == DEVICE_LIST_END {
            in_device_list = false;
            continue;
        }
        if !in_device_list {
            continue;
        }
        if let Some(name) = parse_feature_key(trimmed) {
            device_features.push(name);
        }
    }

    if device_features.is_empty() {
        bail!(
            "no device features found between {DEVICE_LIST_START} and {DEVICE_LIST_END} in Cargo.toml"
        );
    }

    Ok(device_features)
}

fn parse_feature_key(line: &str) -> Option<String> {
    let line = line.split('#').next()?.trim();
    if line.is_empty() {
        return None;
    }
    let (name, _) = line.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_owned())
}

/// Returns `Ok(())` when at least one device-list feature is enabled.
///
/// # Errors
///
/// Returns an error listing available `--features` options and currently
/// enabled features when no device feature is active.
pub fn ensure_device_feature(
    manifest_content: &str,
    cfg_feature: Option<&str>,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Result<()> {
    let device_features = parse_device_features(manifest_content)?;
    let enabled = collect_enabled_feature_names(cfg_feature, vars);

    if any_named_feature_enabled(&enabled, &device_features) {
        return Ok(());
    }

    let options = device_features
        .iter()
        .map(|feature| format!("  --features {feature}"))
        .collect::<Vec<_>>()
        .join("\n");

    bail!(
        "cadmus-core requires a device feature\n\n\
         enable one of:\n{options}\n\n\
         currently enabled features: {}",
        format_enabled_features(&enabled)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MANIFEST: &str = r#"
[package]
name = "cadmus-core"

[features]
default = []
device = []
# device-list-start
emulator = ["sdl2", "device"]
deviceless = ["device"]
kobo = ["procfs", "device"]
# device-list-end
test = []
tracing = []
"#;

    #[test]
    fn parse_device_features_reads_device_list() {
        let features = parse_device_features(SAMPLE_MANIFEST).unwrap();
        assert_eq!(
            features,
            vec![
                "emulator".to_owned(),
                "deviceless".to_owned(),
                "kobo".to_owned()
            ]
        );
    }

    #[test]
    fn parse_device_features_ignores_comments_on_feature_lines() {
        let manifest = r#"
[features]
# device-list-start
kobo = ["device"] # primary device
# device-list-end
"#;
        let features = parse_device_features(manifest).unwrap();
        assert_eq!(features, vec!["kobo".to_owned()]);
    }

    #[test]
    fn parse_device_features_errors_when_markers_missing() {
        let manifest = r#"
[features]
default = []
"#;
        let error = parse_device_features(manifest).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no device features found between")
        );
    }

    #[test]
    fn ensure_device_feature_ok_when_device_enabled_via_cfg() {
        ensure_device_feature(SAMPLE_MANIFEST, Some("emulator,kobo"), std::iter::empty()).unwrap();
    }

    #[test]
    fn ensure_device_feature_ok_when_device_enabled_via_env_var() {
        let vars = [("CARGO_FEATURE_KOBO".to_string(), "1".to_string())];
        ensure_device_feature(SAMPLE_MANIFEST, None, vars).unwrap();
    }

    #[test]
    fn ensure_device_feature_err_when_no_device_feature() {
        let vars = [("CARGO_FEATURE_TRACING".to_string(), "1".to_string())];
        let error = ensure_device_feature(SAMPLE_MANIFEST, None, vars).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cadmus-core requires a device feature")
        );
    }

    #[test]
    fn ensure_device_feature_err_lists_options() {
        let error = ensure_device_feature(SAMPLE_MANIFEST, Some("tracing"), std::iter::empty())
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("--features kobo"));
        assert!(message.contains("--features emulator"));
    }

    #[test]
    fn ensure_device_feature_err_shows_enabled_features() {
        let error = ensure_device_feature(SAMPLE_MANIFEST, Some("tracing"), std::iter::empty())
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("currently enabled features: tracing")
        );
    }
}
