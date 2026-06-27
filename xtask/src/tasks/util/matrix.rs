//! Feature matrix generation for `cargo xtask test` and `cargo xtask clippy`.
//!
//! Scans every `Cargo.toml` in the workspace, collects the union of all
//! non-excluded feature names (see [`is_excluded_feature`]), and produces one
//! CI job per device feature crossed with every power-set combination of the
//! remaining features. Device features are read from the `# device-list-start`
//! block in `crates/core/Cargo.toml` via [`build_deps::manifest`].
//!
//! The same entries are used locally (by `test` and `clippy` tasks) and in CI
//! (by `cargo xtask ci matrix`, which serialises them to GitHub Actions JSON).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Operating systems included in the CI clippy matrix.
pub const CI_CLIPPY_OS: &[&str] = &["ubuntu-latest"];

/// One entry in the feature × OS matrix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatrixEntry {
    /// Human-readable label used in log output and CI job names.
    pub label: String,
    /// Comma-separated feature list passed to `--features`, or empty for the
    /// default build.
    pub features: String,
    /// GitHub Actions runner OS (e.g. `ubuntu-latest`, `macos-latest`).
    pub os: String,
}

impl MatrixEntry {
    /// Returns the `cargo` arguments for this entry (excluding the subcommand).
    ///
    /// For clippy, callers append `-- -D warnings`; for test, callers prepend
    /// `nextest run` or `test --doc`.
    pub fn cargo_args(&self) -> Vec<&str> {
        let mut args = vec!["--workspace"];
        if !self.features.is_empty() {
            args.extend_from_slice(&["--features", &self.features]);
        }
        args
    }
}

/// Scans the workspace and returns the full feature × OS matrix.
///
/// Each entry enables exactly one device feature from the core manifest
/// device-list plus a power-set combination of all other workspace features
/// (excluding `default`, `bench`, `telemetry`, the `device` parent feature,
/// and device-list features).
///
/// # Errors
///
/// Returns an error if the workspace root cannot be located or any
/// `Cargo.toml` cannot be read or parsed.
pub fn scan(root: &Path, os_list: &[&str]) -> Result<Vec<MatrixEntry>> {
    let device_features = device_features_from_core_manifest(root)?;
    let all_features = collect_workspace_features(root)?;
    let device_set: BTreeSet<String> = device_features.iter().cloned().collect();
    let non_device_features = all_features
        .into_iter()
        .filter(|feature| !device_set.contains(feature) && feature != "device")
        .collect();
    Ok(build_device_matrix(
        &device_features,
        non_device_features,
        os_list,
    ))
}

/// Serialises the matrix to a JSON shape for the `test` job.
///
/// The output includes `os` so the workflow can use only `include` entries
/// without relying on matrix cross-product behavior:
///
/// ```json
/// {"include": [{"label": "default", "features": "", "os": "ubuntu-latest"}, ...]}
/// ```
///
/// # Errors
///
/// Returns an error if JSON serialisation fails.
pub fn to_github_test_matrix_json(entries: &[MatrixEntry]) -> Result<String> {
    #[derive(Serialize)]
    struct TestEntry<'a> {
        label: &'a str,
        features: &'a str,
        os: &'a str,
    }

    #[derive(Serialize)]
    struct GithubMatrix<'a> {
        include: Vec<TestEntry<'a>>,
    }

    let include: Vec<TestEntry<'_>> = entries
        .iter()
        .filter(|e| e.os == "ubuntu-latest")
        .map(|e| TestEntry {
            label: &e.label,
            features: &e.features,
            os: &e.os,
        })
        .collect();

    serde_json::to_string(&GithubMatrix { include })
        .context("failed to serialise test matrix to JSON")
}

/// Serialises the matrix to the JSON shape GitHub Actions expects for a
/// dynamic matrix:
///
/// ```json
/// {"include": [{"label": "default", "features": ""}, ...]}
/// ```
///
/// Write this to `$GITHUB_OUTPUT` as `matrix=<json>` to consume it with
/// `fromJson(needs.<job>.outputs.matrix)`.
///
/// # Errors
///
/// Returns an error if JSON serialisation fails.
pub fn to_github_matrix_json(entries: &[MatrixEntry]) -> Result<String> {
    #[derive(Serialize)]
    struct GithubMatrix<'a> {
        include: &'a [MatrixEntry],
    }

    serde_json::to_string(&GithubMatrix { include: entries })
        .context("failed to serialise matrix to JSON")
}

/// Normalises a `--features` argument to the label format used by the matrix.
///
/// Accepts both the comma-separated cargo format (`"tracing,test"`) and the
/// human-readable label format (`"test + tracing"`), sorts the parts
/// alphabetically, and joins them with `" + "`.  An empty input returns
/// `"default"`.
///
/// # Examples
///
/// ```
/// use xtask_lib::tasks::util::matrix::normalize_features_arg;
///
/// assert_eq!(normalize_features_arg("tracing,test"), "test + tracing");
/// assert_eq!(normalize_features_arg("test + tracing"), "test + tracing");
/// assert_eq!(normalize_features_arg("test,tracing"), "test + tracing");
/// assert_eq!(normalize_features_arg(""), "default");
/// assert_eq!(normalize_features_arg("  "), "default");
/// assert_eq!(normalize_features_arg(",,"), "default");
/// assert_eq!(normalize_features_arg("+"), "default");
/// assert_eq!(normalize_features_arg("tracing"), "tracing");
/// ```
pub fn normalize_features_arg(input: &str) -> String {
    let mut parts: Vec<&str> = input
        .split([',', '+'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() {
        return "default".to_owned();
    }

    parts.sort_unstable();
    parts.join(" + ")
}

fn collect_workspace_features(root: &Path) -> Result<BTreeSet<String>> {
    let mut features = BTreeSet::new();

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
    {
        let entry = entry.context("failed to walk workspace")?;
        if entry.file_name() != "Cargo.toml" {
            continue;
        }

        let content = std::fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;

        let doc: toml::Table = toml::from_str(&content)
            .with_context(|| format!("failed to parse {}", entry.path().display()))?;

        if let Some(toml::Value::Table(feat_table)) = doc.get("features") {
            for key in feat_table.keys() {
                if !is_excluded_feature(key) {
                    features.insert(key.clone());
                }
            }
        }
    }

    Ok(features)
}

fn is_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    matches!(name.as_ref(), "target" | ".git" | "thirdparty" | "xtask")
}

/// Returns `true` for feature names that must not appear in the CI matrix.
///
/// - `default` is always enabled by Cargo automatically.
/// - `bench` only changes module visibility for micro-benchmarks and does not
///   need its own power-set of CI jobs.
/// - `telemetry` only aliases `tracing + profiling`, so adding it to the
///   powerset would duplicate combinations that already compile the same code.
fn is_excluded_feature(name: &str) -> bool {
    matches!(name, "default" | "bench" | "docs" | "telemetry")
}

fn device_features_from_core_manifest(root: &Path) -> Result<Vec<String>> {
    let manifest = root.join("crates/core/Cargo.toml");
    let content = std::fs::read_to_string(&manifest)
        .with_context(|| format!("failed to read {}", manifest.display()))?;
    build_deps::manifest::parse_device_features(&content)
}

fn build_device_matrix(
    device_features: &[String],
    non_device_features: BTreeSet<String>,
    os_list: &[&str],
) -> Vec<MatrixEntry> {
    let ancillary: Vec<String> = non_device_features.into_iter().collect();
    let n = ancillary.len();
    let mut entries = Vec::with_capacity(device_features.len() * (1 << n) * os_list.len());

    for device in device_features {
        for mask in 0u32..(1 << n) {
            let extra: Vec<&str> = (0..n)
                .filter(|&i| mask & (1 << i) != 0)
                .map(|i| ancillary[i].as_str())
                .collect();

            let mut label_parts = vec![device.as_str()];
            label_parts.extend(&extra);
            let label = label_parts.join(" + ");

            let mut feature_parts = vec![device.as_str()];
            feature_parts.extend(&extra);
            if !is_valid_device_combo(device, &extra) {
                continue;
            }
            let features = feature_parts.join(",");

            for os in os_list {
                entries.push(MatrixEntry {
                    label: label.clone(),
                    features: features.clone(),
                    os: os.to_string(),
                });
            }
        }
    }

    entries
}

/// Returns `false` for feature combinations that `cadmus-core` rejects at compile time.
fn is_valid_device_combo(device: &str, ancillary: &[&str]) -> bool {
    if ancillary.contains(&"test") {
        return matches!(device, "kobo" | "emulator");
    }
    true
}

#[cfg(test)]
fn build_matrix(features: BTreeSet<String>, os_list: &[&str]) -> Vec<MatrixEntry> {
    let features: Vec<String> = features.into_iter().collect();
    let n = features.len();
    let mut entries = Vec::with_capacity((1 << n) * os_list.len());

    for mask in 0u32..(1 << n) {
        let combo: Vec<&str> = (0..n)
            .filter(|&i| mask & (1 << i) != 0)
            .map(|i| features[i].as_str())
            .collect();

        let label = if combo.is_empty() {
            "default".to_owned()
        } else {
            combo.join(" + ")
        };

        for os in os_list {
            entries.push(MatrixEntry {
                label: label.clone(),
                features: combo.join(","),
                os: os.to_string(),
            });
        }
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::util::workspace;

    const ONE_OS: &[&str] = &["ubuntu-latest"];

    #[test]
    fn build_matrix_empty_features_yields_one_entry_per_os() {
        let entries = build_matrix(BTreeSet::new(), ONE_OS);
        assert_eq!(entries.len(), 1);
        assert!(entries.iter().all(|e| e.label == "default"));
        assert!(entries.iter().all(|e| e.features.is_empty()));
        let oses: Vec<&str> = entries.iter().map(|e| e.os.as_str()).collect();
        assert!(oses.contains(&"ubuntu-latest"));
    }

    #[test]
    fn build_matrix_single_feature_yields_two_combos_per_os() {
        let features = BTreeSet::from(["tracing".to_owned()]);
        let entries = build_matrix(features, ONE_OS);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "default");
        assert_eq!(entries[1].label, "tracing");
    }

    #[test]
    fn build_matrix_two_features_yields_four_combos_per_os() {
        let features = BTreeSet::from(["test".to_owned(), "tracing".to_owned()]);
        let entries = build_matrix(features, ONE_OS);
        assert_eq!(entries.len(), 4);
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"tracing"));
        assert!(labels.contains(&"test"));
        assert!(labels.contains(&"test + tracing"));
    }

    #[test]
    fn build_device_matrix_one_device_and_two_ancillary_yields_four_entries() {
        let device_features = vec!["kobo".to_owned()];
        let ancillary = BTreeSet::from(["test".to_owned(), "tracing".to_owned()]);
        let entries = build_device_matrix(&device_features, ancillary, ONE_OS);
        assert_eq!(entries.len(), 4);
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"kobo"));
        assert!(labels.contains(&"kobo + test"));
        assert!(labels.contains(&"kobo + tracing"));
        assert!(labels.contains(&"kobo + test + tracing"));
    }

    #[test]
    fn build_device_matrix_three_devices_and_three_ancillary_yields_twenty_four_entries() {
        let device_features = vec![
            "emulator".to_owned(),
            "deviceless".to_owned(),
            "kobo".to_owned(),
        ];
        let ancillary = BTreeSet::from([
            "profiling".to_owned(),
            "test".to_owned(),
            "tracing".to_owned(),
        ]);
        let entries = build_device_matrix(&device_features, ancillary, ONE_OS);
        assert_eq!(
            entries.len(),
            20,
            "3 devices × 2³ ancillary − 4 invalid deviceless+test combos × 1 OS = 20 entries"
        );
    }

    #[test]
    fn build_matrix_three_features_one_os_yields_eight_entries() {
        let features = BTreeSet::from([
            "emulator".to_owned(),
            "tracing".to_owned(),
            "test".to_owned(),
        ]);
        let entries = build_matrix(features, ONE_OS);
        assert_eq!(entries.len(), 8, "2³ combos × 1 OS = 8 entries");
    }

    #[test]
    fn cargo_args_default_entry_has_no_features_flag() {
        let entry = MatrixEntry {
            label: "default".to_owned(),
            features: String::new(),
            os: "ubuntu-latest".to_owned(),
        };
        assert_eq!(entry.cargo_args(), vec!["--workspace"]);
    }

    #[test]
    fn cargo_args_combo_entry_includes_features_flag() {
        let entry = MatrixEntry {
            label: "test + tracing".to_owned(),
            features: "test,tracing".to_owned(),
            os: "ubuntu-latest".to_owned(),
        };
        assert_eq!(
            entry.cargo_args(),
            vec!["--workspace", "--features", "test,tracing"]
        );
    }

    #[test]
    fn excludes_telemetry_alias_from_matrix() {
        assert!(is_excluded_feature("telemetry"));
    }

    #[test]
    fn to_github_matrix_json_produces_include_key_with_os() {
        let entries = vec![MatrixEntry {
            label: "default".to_owned(),
            features: String::new(),
            os: "ubuntu-latest".to_owned(),
        }];
        let json = to_github_matrix_json(&entries).unwrap();
        assert!(json.contains("\"include\""));
        assert!(json.contains("\"default\""));
        assert!(json.contains("\"ubuntu-latest\""));
    }

    #[test]
    fn normalize_single_feature() {
        assert_eq!(normalize_features_arg("tracing"), "tracing");
    }

    #[test]
    fn normalize_comma_separated_two_features() {
        assert_eq!(normalize_features_arg("tracing,test"), "test + tracing");
    }

    #[test]
    fn normalize_label_format_round_trips() {
        assert_eq!(normalize_features_arg("test + tracing"), "test + tracing");
    }

    #[test]
    fn normalize_out_of_order_comma_separated_sorts() {
        assert_eq!(normalize_features_arg("tracing,test"), "test + tracing");
    }

    #[test]
    fn normalize_empty_string_returns_default() {
        assert_eq!(normalize_features_arg(""), "default");
    }

    #[test]
    fn scan_workspace_includes_runtime_features_on_ubuntu() {
        let root = workspace::root().expect("workspace root must be resolvable in tests");
        let entries = scan(&root, ONE_OS).expect("scan must succeed");

        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"kobo"));
        assert!(labels.contains(&"emulator + test"));
        assert!(labels.contains(&"deviceless + profiling"));
        assert!(labels.contains(&"kobo + profiling + test + tracing"));
        assert!(!labels.contains(&"default"));
        assert!(!labels.contains(&"telemetry"));
        assert!(
            !labels.iter().any(|label| {
                let parts: Vec<_> = label.split(" + ").collect();
                let device_count = parts
                    .iter()
                    .filter(|part| matches!(**part, "kobo" | "emulator" | "deviceless"))
                    .count();
                device_count > 1
            }),
            "matrix must not combine multiple device features in one entry"
        );
        assert_eq!(
            entries.len(),
            20,
            "3 device features × 2³ ancillary combos − 4 invalid deviceless+test × 1 OS = 20 entries"
        );
    }
}
