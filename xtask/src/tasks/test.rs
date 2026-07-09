//! `cargo xtask test` — run tests across the full feature matrix.
//!
//! The feature matrix is derived dynamically from the workspace `Cargo.toml`
//! files, so adding a new non-aliased feature flag automatically includes it
//! in all test runs without any manual update.
//!
//! Each matrix entry runs two passes:
//! 1. `cargo nextest run` — parallel test execution with per-test output.
//! 2. `cargo test --doc` — doctests, which nextest does not execute.
//!
//! With `--coverage`, nextest runs under `cargo llvm-cov` instrumentation;
//! doctests always use plain `cargo test --doc`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use super::util::{cmd, matrix, workspace};

/// Paths excluded from coverage reports (vendored code, build scripts).
const COVERAGE_IGNORE_RE: &str = r"(thirdparty/|mupdf_wrapper/|build\.rs)";

/// Default local LCOV output (consumed by devenv `cadmus-coverage-*` scripts).
const LOCAL_LCOV_PATH: &str = "target/coverage/lcov.info";

/// JUnit report path when nextest runs with `--profile ci`.
const NEXTEST_JUNIT_PATH: &str = "target/nextest/ci/junit.xml";

/// Arguments for `cargo xtask test`.
#[derive(Debug, Args)]
pub struct TestArgs {
    /// Run only the named feature combination (e.g. `"telemetry + test"`).
    ///
    /// When omitted, all matrix entries are run in sequence.
    #[arg(long)]
    pub features: Option<String>,

    /// Wrap nextest and doctests with `cargo llvm-cov` instrumentation.
    #[arg(long)]
    pub coverage: bool,

    /// Write a Codecov-format JSON report to this path (requires `--coverage`).
    ///
    /// When `--coverage` is set without this flag, writes [`LOCAL_LCOV_PATH`]
    /// for local diff-cover / llvm-cov HTML workflows.
    #[arg(long)]
    pub save_coverage: Option<PathBuf>,

    /// Copy the nextest JUnit report (`target/nextest/ci/junit.xml`) to this path.
    ///
    /// Enables the `ci` nextest profile, which writes JUnit XML for Codecov Test
    /// Analytics. Intended for CI — not enabled by default locally.
    #[arg(long)]
    pub save_junit: Option<PathBuf>,
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
    if args.save_coverage.is_some() && !args.coverage {
        bail!("`--save-coverage` requires `--coverage`");
    }

    let root = workspace::root()?;
    let root_str = root.to_string_lossy().into_owned();
    let env = [("TEST_ROOT_DIR", root_str.as_str())];

    let entries = matrix::scan(&root, &["local"])?;
    let entries = filter(&entries, args.features.as_deref())?;

    let junit = args.save_junit.is_some();

    for entry in &entries {
        run_entry(
            &root,
            &env,
            entry,
            args.coverage,
            junit,
            args.save_junit.as_deref(),
        )?;
    }

    if args.coverage {
        match &args.save_coverage {
            Some(path) => save_coverage_report(&root, path, "--codecov")?,
            None => save_coverage_report(&root, Path::new(LOCAL_LCOV_PATH), "--lcov")?,
        }
    }

    Ok(())
}

/// Runs nextest and doctests for a single matrix entry.
fn run_entry(
    root: &Path,
    env: &[(&str, &str)],
    entry: &matrix::MatrixEntry,
    coverage: bool,
    junit: bool,
    save_junit: Option<&Path>,
) -> Result<()> {
    println!("\n==> nextest ({})", entry.label);
    let nextest_result = run_nextest(root, env, entry, coverage, junit);
    if let Some(dest) = save_junit {
        match save_junit_report(root, dest) {
            Ok(()) => {}
            Err(e) if nextest_result.is_ok() => return Err(e),
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    nextest_result?;

    println!("\n==> doctest ({})", entry.label);
    run_plain_doctests(root, env, entry)?;

    Ok(())
}

fn run_nextest(
    root: &Path,
    env: &[(&str, &str)],
    entry: &matrix::MatrixEntry,
    coverage: bool,
    junit: bool,
) -> Result<()> {
    let mut args = if coverage {
        llvm_cov_nextest_args(junit)
    } else {
        plain_nextest_args(junit)
    };
    args.extend(entry.cargo_args().into_iter().map(str::to_owned));
    run_cargo(&args, root, env)
}

fn llvm_cov_nextest_args(junit: bool) -> Vec<String> {
    let mut args = vec![
        "llvm-cov".to_owned(),
        "--ignore-filename-regex".to_owned(),
        COVERAGE_IGNORE_RE.to_owned(),
        "nextest".to_owned(),
        "--all-targets".to_owned(),
    ];
    if junit {
        args.extend(["--profile".to_owned(), "ci".to_owned()]);
    }
    args
}

fn plain_nextest_args(junit: bool) -> Vec<String> {
    let mut args = vec![
        "nextest".to_owned(),
        "run".to_owned(),
        "--all-targets".to_owned(),
    ];
    if junit {
        args.extend(["--profile".to_owned(), "ci".to_owned()]);
    }
    args
}

fn save_junit_report(root: &Path, dest: &Path) -> Result<()> {
    let src = root.join(NEXTEST_JUNIT_PATH);
    if !src.is_file() {
        bail!(
            "nextest JUnit report missing at {} (is --profile ci configured in .config/nextest.toml?)",
            src.display()
        );
    }
    ensure_parent_dir(dest)?;
    std::fs::copy(&src, dest)
        .with_context(|| format!("failed to copy {} to {}", src.display(), dest.display()))?;
    Ok(())
}

/// Runs `cargo test --doc` for a single matrix entry.
///
/// doctest does not support coverage instrumentation, so we run it without it.
fn run_plain_doctests(
    root: &Path,
    env: &[(&str, &str)],
    entry: &matrix::MatrixEntry,
) -> Result<()> {
    let mut args = vec!["test".to_owned(), "--doc".to_owned()];
    args.extend(entry.cargo_args().into_iter().map(str::to_owned));
    run_cargo(&args, root, env)
}

fn run_cargo(args: &[String], root: &Path, env: &[(&str, &str)]) -> Result<()> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    cmd::run("cargo", &arg_refs, root, env)
}

fn save_coverage_report(root: &Path, output: &Path, format_flag: &str) -> Result<()> {
    ensure_parent_dir(output)?;
    let output_str = output.to_string_lossy();
    run_cargo(
        &[
            "llvm-cov".to_owned(),
            "report".to_owned(),
            format_flag.to_owned(),
            "--output-path".to_owned(),
            output_str.into_owned(),
            "--ignore-filename-regex".to_owned(),
            COVERAGE_IGNORE_RE.to_owned(),
        ],
        root,
        &[],
    )
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    Ok(())
}

/// Returns the matrix entries to run, optionally filtered by label.
///
/// When `label` is `None` all entries are returned.  When a label is
/// provided it is normalised via [`matrix::normalize_features_arg`] before
/// matching, so both `"telemetry,test"` and `"telemetry + test"` resolve to
/// the same
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_nextest_args_match_cargo_nextest_run() {
        assert_eq!(
            plain_nextest_args(false),
            vec![
                "nextest".to_owned(),
                "run".to_owned(),
                "--all-targets".to_owned(),
            ]
        );
    }

    #[test]
    fn plain_nextest_args_with_junit_use_ci_profile() {
        assert_eq!(
            plain_nextest_args(true),
            vec![
                "nextest".to_owned(),
                "run".to_owned(),
                "--all-targets".to_owned(),
                "--profile".to_owned(),
                "ci".to_owned(),
            ]
        );
    }

    #[test]
    fn llvm_cov_nextest_args_omit_run_subcommand() {
        assert_eq!(
            llvm_cov_nextest_args(false),
            vec![
                "llvm-cov".to_owned(),
                "--ignore-filename-regex".to_owned(),
                COVERAGE_IGNORE_RE.to_owned(),
                "nextest".to_owned(),
                "--all-targets".to_owned(),
            ]
        );
    }

    #[test]
    fn llvm_cov_nextest_args_with_junit_use_ci_profile() {
        assert_eq!(
            llvm_cov_nextest_args(true),
            vec![
                "llvm-cov".to_owned(),
                "--ignore-filename-regex".to_owned(),
                COVERAGE_IGNORE_RE.to_owned(),
                "nextest".to_owned(),
                "--all-targets".to_owned(),
                "--profile".to_owned(),
                "ci".to_owned(),
            ]
        );
    }
}
