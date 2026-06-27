//! Helpers for collecting enabled Cargo features from build-script env vars.

const CARGO_FEATURE_PREFIX: &str = "CARGO_FEATURE_";

/// Parses a `CARGO_CFG_FEATURE` value into normalized feature names.
pub fn parse_cfg_feature_list(value: &str) -> Vec<String> {
    let mut features = value
        .split(',')
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    features.sort();
    features
}

/// Collects enabled Cargo feature names from build-script environment variables.
pub fn collect_cargo_feature_names(
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<String> {
    let mut features = vars
        .into_iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(CARGO_FEATURE_PREFIX)
                .filter(|_| value == "1")
                .map(|name| name.to_ascii_lowercase().replace('_', "-"))
        })
        .collect::<Vec<_>>();
    features.sort();
    features
}

/// Collects enabled feature names, preferring `CARGO_CFG_FEATURE` when set.
pub fn collect_enabled_feature_names(
    cfg_feature: Option<&str>,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<String> {
    if let Some(cfg_feature) = cfg_feature {
        let features = parse_cfg_feature_list(cfg_feature);
        if !features.is_empty() {
            return features;
        }
    }

    collect_cargo_feature_names(vars)
}

/// Formats enabled feature names for error messages.
pub fn format_enabled_features(features: &[String]) -> String {
    if features.is_empty() {
        "(none)".to_owned()
    } else {
        features.join(", ")
    }
}

/// Returns true when any candidate feature appears in `enabled`.
pub fn any_named_feature_enabled(enabled: &[String], candidates: &[String]) -> bool {
    candidates
        .iter()
        .any(|candidate| enabled.iter().any(|feature| feature == candidate))
}

/// Returns the `CARGO_FEATURE_*` env var name to watch for `feature`.
pub fn cargo_feature_env_key(feature: &str) -> String {
    format!(
        "{CARGO_FEATURE_PREFIX}{}",
        feature.to_ascii_uppercase().replace('-', "_")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cfg_feature_list_trims_and_skips_empty() {
        assert_eq!(
            parse_cfg_feature_list(" kobo , , tracing "),
            vec!["kobo".to_string(), "tracing".to_string()]
        );
    }

    #[test]
    fn collect_cargo_feature_names_sorts_and_normalizes() {
        let vars = [
            ("CARGO_FEATURE_KOBO".to_string(), "1".to_string()),
            ("CARGO_FEATURE_TEST".to_string(), "1".to_string()),
            ("CARGO_FEATURE_FOO_BAR".to_string(), "1".to_string()),
            ("CARGO_FEATURE_DISABLED".to_string(), "0".to_string()),
            ("OTHER_VAR".to_string(), "1".to_string()),
        ];
        assert_eq!(
            collect_cargo_feature_names(vars),
            vec![
                "foo-bar".to_string(),
                "kobo".to_string(),
                "test".to_string()
            ]
        );
    }

    #[test]
    fn collect_cargo_feature_names_returns_empty_when_none_enabled() {
        assert!(collect_cargo_feature_names(std::iter::empty()).is_empty());
    }

    #[test]
    fn collect_enabled_feature_names_prefers_cfg() {
        let vars = [("CARGO_FEATURE_KOBO".to_string(), "1".to_string())];
        assert_eq!(
            collect_enabled_feature_names(Some("tracing"), vars),
            vec!["tracing".to_string()]
        );
    }

    #[test]
    fn collect_enabled_feature_names_falls_back_to_env_vars() {
        let vars = [("CARGO_FEATURE_KOBO".to_string(), "1".to_string())];
        assert_eq!(
            collect_enabled_feature_names(None, vars),
            vec!["kobo".to_string()]
        );
    }

    #[test]
    fn format_enabled_features_empty() {
        assert_eq!(format_enabled_features(&[]), "(none)");
    }

    #[test]
    fn format_enabled_features_joins() {
        assert_eq!(
            format_enabled_features(&["kobo".to_string(), "tracing".to_string()]),
            "kobo, tracing"
        );
    }

    #[test]
    fn any_named_feature_enabled_matches() {
        let enabled = vec!["tracing".to_string(), "test".to_string()];
        let candidates = vec!["kobo".to_string(), "test".to_string()];
        assert!(any_named_feature_enabled(&enabled, &candidates));
        assert!(!any_named_feature_enabled(
            &enabled,
            &["kobo".to_string(), "emulator".to_string()]
        ));
    }

    #[test]
    fn cargo_feature_env_key_normalizes_hyphens() {
        assert_eq!(cargo_feature_env_key("foo-bar"), "CARGO_FEATURE_FOO_BAR");
    }
}
