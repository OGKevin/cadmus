//! `cargo xtask test` — run tests across the full feature matrix.
//!
//! The feature matrix is derived dynamically from the workspace `Cargo.toml`
//! files, so adding a new feature flag automatically includes it in all test
//! runs without any manual update.
//!
//! Each matrix entry runs two passes:
//! 1. `cargo nextest run` — parallel test execution with per-test output.
//! 2. `cargo test --doc` — doctests, which nextest does not execute.

use anyhow::{Result, bail};
use clap::Args;

use super::util::{cmd, matrix, workspace};

/// Arguments for `cargo xtask test`.
#[derive(Debug, Args)]
pub struct TestArgs {
    /// Run only the named feature combination (e.g. `"test + otel"`).
    ///
    /// When omitted, all matrix entries are run in sequence.
    #[arg(long)]
    pub features: Option<String>,
}

/// Runs `cargo nextest run` and `cargo test --doc` across the feature matrix
/// (or a single entry).
///
/// The `TEST_ROOT_DIR` environment variable is set to the workspace root so
/// that integration tests that read fixture files can locate them regardless
/// of the working directory.
///
/// # Errors
///
/// Returns the first test failure encountered.
pub fn run(args: TestArgs) -> Result<()> {
    let root = workspace::root()?;
    let root_str = root.to_string_lossy().into_owned();
    let env = [("TEST_ROOT_DIR", root_str.as_str())];

    let entries = matrix::scan(&root, &["local"])?;
    let entries = filter(&entries, args.features.as_deref())?;

    for entry in entries {
        println!("\n==> nextest ({})", entry.label);

        let mut nextest_args = vec!["nextest", "run", "--all-targets"];
        nextest_args.extend_from_slice(&entry.cargo_args());
        cmd::run("cargo", &nextest_args, &root, &env)?;

        println!("\n==> doctest ({})", entry.label);

        let mut doctest_args = vec!["test", "--doc"];
        doctest_args.extend_from_slice(&entry.cargo_args());
        cmd::run("cargo", &doctest_args, &root, &env)?;
    }

    Ok(())
}

/// Returns the matrix entries to run, optionally filtered by label.
///
/// When `label` is `None` all entries are returned.  When a label is
/// provided it is normalised via [`matrix::normalize_features_arg`] before
/// matching, so both `"otel,test"` and `"otel + test"` resolve to the same
/// entry.  An unknown label after normalisation is an error.
///
/// # Errors
///
/// Returns an error when a label is provided but no matrix entry matches,
/// listing all available labels.
fn filter<'a>(
    entries: &'a [matrix::MatrixEntry],
    label: Option<&str>,
) -> Result<Vec<&'a matrix::MatrixEntry>> {
    let Some(raw) = label else {
        return Ok(entries.iter().collect());
    };

    let normalised = matrix::normalize_features_arg(raw);
    let matched: Vec<&matrix::MatrixEntry> =
        entries.iter().filter(|e| e.label == normalised).collect();

    if matched.is_empty() {
        let available: Vec<&str> = entries
            .iter()
            .map(|e| e.label.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        bail!(
            "unknown feature combination {:?} (normalised to {:?})\n\nAvailable labels:\n  {}",
            raw,
            normalised,
            available.join("\n  ")
        );
    }

    Ok(matched)
}
