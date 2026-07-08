//! Helpers for collecting enabled Cargo features from build-script env vars.

const CARGO_FEATURE_PREFIX: &str = "CARGO_FEATURE_";

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
    fn cargo_feature_env_key_normalizes_hyphens() {
        assert_eq!(cargo_feature_env_key("foo-bar"), "CARGO_FEATURE_FOO_BAR");
    }
}
