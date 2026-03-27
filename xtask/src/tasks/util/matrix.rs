//! Feature matrix generation for `cargo xtask test` and `cargo xtask clippy`.
//!
//! Scans every `Cargo.toml` in the workspace, collects the union of all
//! non-`default` feature names, and produces every power-set combination
//! crossed with a list of target operating systems.  Each combination becomes
//! one [`MatrixEntry`] that maps to a single CI job.
//!
//! The same entries are used locally (by `test` and `clippy` tasks) and in CI
//! (by `cargo xtask ci matrix`, which serialises them to GitHub Actions JSON).

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

/// Operating systems included in the CI clippy matrix.
pub const CI_CLIPPY_OS: &[&str] = &["ubuntu-latest", "macos-latest"];

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
/// Features named `default` are excluded — Cargo enables them automatically.
/// The matrix always starts with the default (no explicit features) entry,
/// followed by every non-empty power-set combination in a stable order.
/// Each feature combination is repeated once per OS in `os_list`.
///
/// # Errors
///
/// Returns an error if the workspace root cannot be located or any
/// `Cargo.toml` cannot be read or parsed.
pub fn scan(root: &Path, os_list: &[&str]) -> Result<Vec<MatrixEntry>> {
    let features = collect_workspace_features(root)?;
    Ok(build_matrix(features, os_list))
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
/// Accepts both the comma-separated cargo format (`"otel,test"`) and the
/// human-readable label format (`"otel + test"`), sorts the parts
/// alphabetically, and joins them with `" + "`.  An empty input returns
/// `"default"`.
///
/// # Examples
///
/// ```
/// use xtask_lib::tasks::util::matrix::normalize_features_arg;
///
/// assert_eq!(normalize_features_arg("otel,test"), "otel + test");
/// assert_eq!(normalize_features_arg("otel + test"), "otel + test");
/// assert_eq!(normalize_features_arg("test,otel"), "otel + test");
/// assert_eq!(normalize_features_arg(""), "default");
/// assert_eq!(normalize_features_arg("  "), "default");
/// assert_eq!(normalize_features_arg(",,"), "default");
/// assert_eq!(normalize_features_arg("+"), "default");
/// assert_eq!(normalize_features_arg("otel"), "otel");
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
                if key != "default" {
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

    const TWO_OS: &[&str] = &["ubuntu-latest", "macos-latest"];
    const ONE_OS: &[&str] = &["ubuntu-latest"];

    #[test]
    fn build_matrix_empty_features_yields_one_entry_per_os() {
        let entries = build_matrix(BTreeSet::new(), TWO_OS);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.label == "default"));
        assert!(entries.iter().all(|e| e.features.is_empty()));
        let oses: Vec<&str> = entries.iter().map(|e| e.os.as_str()).collect();
        assert!(oses.contains(&"ubuntu-latest"));
        assert!(oses.contains(&"macos-latest"));
    }

    #[test]
    fn build_matrix_single_feature_yields_two_combos_per_os() {
        let features = BTreeSet::from(["otel".to_owned()]);
        let entries = build_matrix(features, ONE_OS);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "default");
        assert_eq!(entries[1].label, "otel");
    }

    #[test]
    fn build_matrix_two_features_yields_four_combos_per_os() {
        let features = BTreeSet::from(["otel".to_owned(), "test".to_owned()]);
        let entries = build_matrix(features, ONE_OS);
        assert_eq!(entries.len(), 4);
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"otel"));
        assert!(labels.contains(&"test"));
        assert!(labels.contains(&"otel + test"));
    }

    #[test]
    fn build_matrix_three_features_two_os_yields_sixteen_entries() {
        let features =
            BTreeSet::from(["emulator".to_owned(), "otel".to_owned(), "test".to_owned()]);
        let entries = build_matrix(features, TWO_OS);
        assert_eq!(entries.len(), 16, "2³ combos × 2 OSes = 16 entries");
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
            label: "test + otel".to_owned(),
            features: "test,otel".to_owned(),
            os: "ubuntu-latest".to_owned(),
        };
        assert_eq!(
            entry.cargo_args(),
            vec!["--workspace", "--features", "test,otel"]
        );
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
        assert_eq!(normalize_features_arg("otel"), "otel");
    }

    #[test]
    fn normalize_comma_separated_two_features() {
        assert_eq!(normalize_features_arg("otel,test"), "otel + test");
    }

    #[test]
    fn normalize_label_format_round_trips() {
        assert_eq!(normalize_features_arg("otel + test"), "otel + test");
    }

    #[test]
    fn normalize_out_of_order_comma_separated_sorts() {
        assert_eq!(normalize_features_arg("test,otel"), "otel + test");
    }

    #[test]
    fn normalize_empty_string_returns_default() {
        assert_eq!(normalize_features_arg(""), "default");
    }

    #[test]
    fn scan_workspace_finds_known_features_across_two_os() {
        let root = workspace::root().expect("workspace root must be resolvable in tests");
        let entries = scan(&root, TWO_OS).expect("scan must succeed");

        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"default"));
        assert!(labels.contains(&"emulator"));
        assert!(labels.contains(&"otel"));
        assert!(labels.contains(&"test"));
        assert_eq!(
            entries.len(),
            32,
            "4 features → 2⁴ = 16 combos × 2 OSes = 32 entries"
        );
    }
}
