//! `cargo xtask clippy` — lint across the full feature matrix.
//!
//! The feature matrix is derived dynamically from the workspace `Cargo.toml`
//! files, so adding a new feature flag automatically includes it in all
//! clippy runs without any manual update.
//!
//! ## Feature matrix
//!
//! Every power-set combination of workspace features is checked, each scoped
//! to `--workspace --all-targets`.
//!
//! ## Reviewdog reporting
//!
//! When `--github-report` is passed, clippy output is emitted as JSON and
//! piped through [`reviewdog`](https://github.com/reviewdog/reviewdog).
//! Reviewdog filters the diagnostics to only lines touched by the diff and
//! posts them as inline review comments.
//!
//! Two modes are supported:
//!
//! | Flag | Reporter | Diff source | Use case |
//! |------|----------|-------------|----------|
//! | `--github-report` | `github-pr-review` | GitHub PR API | CI on pull requests |
//! | `--github-report --diff-branch master` | `local` | `git diff master` | Local development |
//!
//! CI requirements:
//! - `reviewdog` on `PATH`
//! - `REVIEWDOG_GITHUB_API_TOKEN` set to a token with `pull-requests: write`
//!
//! Local requirements:
//! - `reviewdog` on `PATH` (provided by the devenv shell)
//!
//! ## JSON artifact collection
//!
//! When `--save-json <path>` is passed, raw clippy JSON lines are written to
//! the given file instead of running reviewdog or applying `-D warnings`.
//! This is used by CI to collect per-feature-label artifacts that are later
//! deduplicated and reported in a single `cargo xtask ci clippy-report` run.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Args;

use super::util::{cmd, matrix, workspace};

/// Arguments for `cargo xtask clippy`.
#[derive(Debug, Args)]
pub struct ClippyArgs {
    /// Run only the named feature combination (e.g. `"test + otel"`).
    ///
    /// When omitted, all matrix entries are checked in sequence.
    #[arg(long)]
    pub features: Option<String>,

    /// Pipe clippy JSON output through reviewdog.
    ///
    /// Without `--diff-branch`, uses `github-pr-review` reporter (CI mode):
    /// reviewdog fetches the PR diff from the GitHub API and posts inline
    /// review comments.  Requires `REVIEWDOG_GITHUB_API_TOKEN`.
    ///
    /// With `--diff-branch`, uses `local` reporter (local dev mode):
    /// reviewdog diffs against the given branch and prints findings to the
    /// terminal.
    #[arg(long)]
    pub github_report: bool,

    /// Branch to diff against when running reviewdog locally.
    ///
    /// When set, reviewdog uses `git diff <branch>` as the diff source and
    /// the `local` reporter instead of posting GitHub review comments.
    /// Typically set to `master`.
    #[arg(long)]
    pub diff_branch: Option<String>,

    /// Write raw clippy JSON lines to this file instead of linting or reporting.
    ///
    /// Skips `-D warnings` and reviewdog.  The file is created (or truncated)
    /// and each JSON line emitted by `cargo clippy --message-format=json` is
    /// written verbatim.  Used by CI to collect per-label artifacts for
    /// `cargo xtask ci clippy-report`.
    #[arg(long)]
    pub save_json: Option<PathBuf>,
}

/// Runs `cargo clippy` across the feature matrix (or a single entry).
///
/// Three modes:
///
/// - **Normal**: runs clippy and exits non-zero only if clippy itself errors.
/// - **Reviewdog** (`--github-report` or `--diff-branch`): pipes JSON output
///   through `reviewdog`.  `--diff-branch` alone is sufficient for local use;
///   `--github-report` alone uses the `github-pr-review` reporter in CI.
/// - **`--save-json <path>`**: writes raw JSON lines to a file for later
///   deduplication by `cargo xtask ci clippy-report`.
///
/// # Errors
///
/// Returns the first clippy error encountered, or a spawn/IO error.
pub fn run(args: ClippyArgs) -> Result<()> {
    let root = workspace::root()?;
    let entries = matrix::scan(&root, &["local"])?;
    let entries = filter(&entries, args.features.as_deref())?;

    for entry in entries {
        println!("\n==> clippy ({})", entry.label);

        if let Some(ref path) = args.save_json {
            save_json(&root, &entry.cargo_args(), path)?;
        } else if args.github_report || args.diff_branch.is_some() {
            run_with_reviewdog(&root, &entry.cargo_args(), args.diff_branch.as_deref())?;
        } else {
            let mut clippy_args = vec!["clippy", "--all-targets"];
            clippy_args.extend_from_slice(&entry.cargo_args());
            cmd::run("cargo", &clippy_args, &root, &[])?;
        }
    }

    Ok(())
}

/// Runs `cargo clippy --message-format=json` and writes every output line to
/// `dest`, creating or truncating the file.
///
/// Does not apply `-D warnings` — the exit status of clippy is ignored so
/// that artifact collection always succeeds even when warnings are present.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or the file cannot be
/// written.
pub(crate) fn save_json(
    root: &std::path::Path,
    cargo_args: &[&str],
    dest: &std::path::Path,
) -> Result<()> {
    let mut clippy_args = vec!["clippy", "--all-targets", "--message-format=json"];
    clippy_args.extend_from_slice(cargo_args);

    println!("$ cargo {}", clippy_args.join(" "));

    let mut child = Command::new("cargo")
        .args(&clippy_args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn `cargo clippy`")?;

    let stdout = child.stdout.take().context("clippy stdout not captured")?;
    let reader = BufReader::new(stdout);

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let mut file =
        fs::File::create(dest).with_context(|| format!("failed to create {}", dest.display()))?;

    for line in reader.lines() {
        let line = line.context("failed to read clippy output")?;
        writeln!(file, "{line}")
            .with_context(|| format!("failed to write to {}", dest.display()))?;
    }

    child.wait().context("failed to wait for `cargo clippy`")?;

    Ok(())
}

/// Runs `cargo clippy --message-format=short` and pipes the output through
/// `reviewdog`.
///
/// When `diff_branch` is `Some`, reviewdog uses `git diff <branch>` as the
/// diff source and the `local` reporter (terminal output).  When `None`,
/// reviewdog fetches the PR diff from the GitHub API and posts inline review
/// comments via the `github-pr-review` reporter.
///
/// Both processes run concurrently via a pipe so neither buffers the full
/// output in memory.
///
/// # Errors
///
/// Returns an error if either process fails to spawn or exits non-zero.
fn run_with_reviewdog(
    root: &std::path::Path,
    cargo_args: &[&str],
    diff_branch: Option<&str>,
) -> Result<()> {
    let mut clippy_args = vec![
        "clippy",
        "--all-targets",
        "--message-format=short",
        "--quiet",
    ];
    clippy_args.extend_from_slice(cargo_args);

    let mut reviewdog_args = vec![
        "-f=clippy".to_owned(),
        "-filter-mode=added".to_owned(),
        "-fail-on-error=false".to_owned(),
    ];

    if let Some(branch) = diff_branch {
        reviewdog_args.push("-reporter=local".to_owned());
        reviewdog_args.push(format!("-diff=git diff --no-ext-diff {branch}"));
    } else {
        reviewdog_args.push("-reporter=github-pr-review".to_owned());
    }

    println!("$ cargo {}", clippy_args.join(" "));
    println!("$ reviewdog {}", reviewdog_args.join(" "));

    let mut clippy = Command::new("sh")
        .args(["-c", &format!("cargo {} 2>&1", clippy_args.join(" "))])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn `cargo clippy`")?;

    let clippy_stdout = clippy.stdout.take().context("clippy stdout not captured")?;

    let mut reviewdog = Command::new("reviewdog")
        .args(&reviewdog_args)
        .current_dir(root)
        .stdin(clippy_stdout)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn `reviewdog` — is it installed and on PATH?")?;

    let clippy_status = clippy.wait().context("failed to wait for `cargo clippy`")?;
    let reviewdog_status = reviewdog.wait().context("failed to wait for `reviewdog`")?;

    if !clippy_status.success() {
        bail!("`cargo clippy` exited with status {}", clippy_status);
    }
    if !reviewdog_status.success() {
        bail!("`reviewdog` exited with status {}", reviewdog_status);
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
