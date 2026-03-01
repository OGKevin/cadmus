//! `cargo xtask ci clippy-report` — deduplicate clippy JSON artifacts and
//! report via reviewdog.
//!
//! ## How it fits into CI
//!
//! The `clippy` matrix job runs `cargo xtask clippy --save-json <file>` for
//! every feature label on `ubuntu-latest`, uploading each file as a GitHub
//! Actions artifact.  After the matrix completes, the `clippy-report` job
//! downloads all artifacts into a single directory and calls:
//!
//! ```text
//! cargo xtask ci clippy-report --artifacts-dir <dir>
//! ```
//!
//! This command reads every `.json` file in the directory, deduplicates
//! diagnostics across feature labels, and pipes the unique set through
//! `reviewdog` once — so each warning appears as exactly one PR review
//! comment regardless of how many feature combinations triggered it.
//!
//! ## Deduplication key
//!
//! Two diagnostics are considered identical when they share the same
//! `(file_name, line_start, message)` triple taken from the primary span of the
//! clippy JSON message.  Only diagnostic messages (compiler-message with spans)
//! are included; non-diagnostic JSON (build artifacts, build-finished, etc.)
//! is filtered out.
//!
//! ## Reviewdog
//!
//! Uses the `github-pr-review` reporter.  Requires:
//! - `reviewdog` on `PATH`
//! - `REVIEWDOG_GITHUB_API_TOKEN` set to a token with `pull-requests: write`

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::Value;

/// Arguments for `cargo xtask ci clippy-report`.
#[derive(Debug, Args)]
pub struct ClippyReportArgs {
    /// Directory containing `.json` artifact files produced by
    /// `cargo xtask clippy --save-json`.
    #[arg(long)]
    pub artifacts_dir: PathBuf,
}

/// Reads all `.json` files in `artifacts_dir`, deduplicates diagnostics, and
/// pipes the unique set through `reviewdog` using the `github-pr-review`
/// reporter.
///
/// # Errors
///
/// Returns an error if any artifact file cannot be read, if `reviewdog`
/// cannot be spawned, or if `reviewdog` exits non-zero.
pub fn run(args: ClippyReportArgs) -> Result<()> {
    let lines = collect_unique_lines(&args.artifacts_dir)?;

    println!(
        "clippy-report: {} unique diagnostics across all feature labels",
        lines.len()
    );

    pipe_to_reviewdog(&lines)
}

/// Collects all JSON lines from every `.json` file in `dir`, returning only
/// the unique ones (deduplicated by primary span + message text).
///
/// Only diagnostic messages (compiler-message with spans) are included.
/// Non-diagnostic JSON (build artifacts, build-finished, etc.) is filtered out.
///
/// # Errors
///
/// Returns an error if the directory cannot be read or any file cannot be
/// opened.
fn collect_unique_lines(dir: &Path) -> Result<Vec<String>> {
    let mut seen: HashSet<DiagnosticKey> = HashSet::new();
    let mut unique: Vec<String> = Vec::new();

    for path in json_files(dir)? {
        let file =
            fs::File::open(&path).with_context(|| format!("failed to open {}", path.display()))?;

        for line in BufReader::new(file).lines() {
            let line = line.with_context(|| format!("failed to read {}", path.display()))?;

            if line.trim().is_empty() {
                continue;
            }

            let key = diagnostic_key(&line);

            if let DiagnosticKey::Spanned { .. } = key
                && seen.insert(key)
            {
                unique.push(line);
            }
        }
    }

    Ok(unique)
}

/// Returns sorted paths of every `.json` file directly inside `dir`.
///
/// Sorting ensures deterministic ordering across runs.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("failed to read directory {}", dir.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    paths.sort();

    Ok(paths)
}

/// A key that uniquely identifies a clippy diagnostic for deduplication.
///
/// Two diagnostics with the same file, line, and message text are considered
/// identical even if they were produced under different feature combinations.
/// Non-diagnostic JSON (e.g., build artifacts) returns a Raw key but is filtered
/// out during collection since only Spanned keys are forwarded to reviewdog.
#[derive(Debug, PartialEq, Eq, Hash)]
enum DiagnosticKey {
    Spanned {
        file: String,
        line: u64,
        message: String,
    },
    Raw(String),
}

/// Extracts a primary span from clippy JSON diagnostic message.
///
/// The spans array can have multiple entries. The primary span is identified
/// by having `is_primary: true`. If no span has this flag, returns the first span.
fn find_primary_span(message: &Value) -> Option<&Value> {
    let spans = message.pointer("/message/spans")?.as_array()?;

    for span in spans {
        if span.get("is_primary").and_then(Value::as_bool) == Some(true) {
            return Some(span);
        }
    }

    spans.first()
}

/// Extracts a [`DiagnosticKey`] from a raw clippy JSON line.
fn diagnostic_key(line: &str) -> DiagnosticKey {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return DiagnosticKey::Raw(line.to_owned());
    };

    let Some(span) = find_primary_span(&value) else {
        return DiagnosticKey::Raw(line.to_owned());
    };

    let file = span
        .get("file_name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let line_start = span.get("line_start").and_then(Value::as_u64);
    let message = value
        .pointer("/message/message")
        .and_then(Value::as_str)
        .map(str::to_owned);

    match (file, line_start, message) {
        (Some(file), Some(line), Some(message)) => DiagnosticKey::Spanned {
            file,
            line,
            message,
        },
        _ => DiagnosticKey::Raw(line.to_owned()),
    }
}

/// Converts a clippy JSON line to short format for reviewdog.
///
/// # Panics
///
/// Panics if the JSON line does not have the expected diagnostic structure
/// (i.e., missing primary span or `/message/message`).
/// Non-diagnostic JSON lines (like `build-finished`) should be filtered out
/// before calling this function.
fn json_to_short(line: &str) -> String {
    let value = serde_json::from_str::<Value>(line).expect("failed to parse JSON line");

    let span = find_primary_span(&value).expect("clippy JSON should have a primary span");

    let file = span
        .get("file_name")
        .and_then(Value::as_str)
        .expect("primary span should have file_name");

    let line_start = span
        .get("line_start")
        .and_then(Value::as_u64)
        .expect("primary span should have line_start");

    let column_start = span
        .get("column_start")
        .and_then(Value::as_u64)
        .unwrap_or(1);

    let level = value
        .pointer("/message/level")
        .and_then(Value::as_str)
        .unwrap_or("warning");

    let message = value
        .pointer("/message/message")
        .and_then(Value::as_str)
        .expect("clippy JSON should have /message/message");

    let code = value.pointer("/message/code/code").and_then(Value::as_str);

    if let Some(code) = code {
        format!("{file}:{line_start}:{column_start}: {level}: {message} [{code}]")
    } else {
        format!("{file}:{line_start}:{column_start}: {level}: {message}")
    }
}

/// Spawns `reviewdog` with the `github-pr-review` reporter and writes `lines`
/// to its stdin.
///
/// JSON lines are converted to short format for compatibility with reviewdog's
/// clippy parser.
///
/// # Errors
///
/// Returns an error if `reviewdog` cannot be spawned or exits non-zero.
fn pipe_to_reviewdog(lines: &[String]) -> Result<()> {
    let reviewdog_args = [
        "-f=clippy",
        "-filter-mode=added",
        "-fail-on-error=false",
        "-reporter=github-pr-review",
    ];

    println!("$ reviewdog {}", reviewdog_args.join(" "));

    let mut reviewdog = Command::new("reviewdog")
        .args(reviewdog_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn `reviewdog` — is it installed and on PATH?")?;

    let mut stdin = reviewdog
        .stdin
        .take()
        .context("reviewdog stdin not captured")?;

    for line in lines {
        let short_line = json_to_short(line);
        writeln!(stdin, "{short_line}").context("failed to write to reviewdog stdin")?;
    }

    drop(stdin);

    let status = reviewdog.wait().context("failed to wait for `reviewdog`")?;

    if !status.success() {
        bail!("`reviewdog` exited with status {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;

    fn write_artifact(dir: &Path, name: &str, lines: &[&str]) {
        let path = dir.join(name);
        let mut f = fs::File::create(path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    fn spanned_line(file: &str, line: u64, message: &str) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": message,
                "spans": [{ "file_name": file, "line_start": line }]
            }
        })
        .to_string()
    }

    #[test]
    fn deduplicates_identical_diagnostics_across_files() {
        let dir = tempdir().unwrap();
        let warning = spanned_line("src/lib.rs", 10, "unused variable");

        write_artifact(dir.path(), "default.json", &[&warning]);
        write_artifact(dir.path(), "test.json", &[&warning]);

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn keeps_diagnostics_with_different_messages() {
        let dir = tempdir().unwrap();

        write_artifact(
            dir.path(),
            "a.json",
            &[&spanned_line("src/lib.rs", 10, "unused variable")],
        );
        write_artifact(
            dir.path(),
            "b.json",
            &[&spanned_line("src/lib.rs", 10, "dead code")],
        );

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn keeps_diagnostics_with_different_lines() {
        let dir = tempdir().unwrap();

        write_artifact(
            dir.path(),
            "a.json",
            &[&spanned_line("src/lib.rs", 10, "unused variable")],
        );
        write_artifact(
            dir.path(),
            "b.json",
            &[&spanned_line("src/lib.rs", 20, "unused variable")],
        );

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn keeps_diagnostics_with_different_files() {
        let dir = tempdir().unwrap();

        write_artifact(
            dir.path(),
            "a.json",
            &[&spanned_line("src/lib.rs", 10, "unused variable")],
        );
        write_artifact(
            dir.path(),
            "b.json",
            &[&spanned_line("src/main.rs", 10, "unused variable")],
        );

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn non_json_lines_are_filtered_out() {
        let dir = tempdir().unwrap();

        write_artifact(dir.path(), "a.json", &["not json"]);
        write_artifact(dir.path(), "b.json", &["not json"]);

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn spanless_json_is_filtered_out() {
        let dir = tempdir().unwrap();
        let spanless = serde_json::json!({ "reason": "build-finished" }).to_string();

        write_artifact(dir.path(), "a.json", &[&spanless]);
        write_artifact(dir.path(), "b.json", &[&spanless]);

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn empty_lines_are_skipped() {
        let dir = tempdir().unwrap();

        write_artifact(dir.path(), "a.json", &["", "  ", ""]);

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert!(lines.is_empty());
    }

    #[test]
    fn ignores_non_json_files_in_directory() {
        let dir = tempdir().unwrap();
        let warning = spanned_line("src/lib.rs", 1, "unused");

        write_artifact(dir.path(), "default.json", &[&warning]);

        fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();

        let lines = collect_unique_lines(dir.path()).unwrap();

        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn json_to_short_converts_clippy_json_to_short_format() {
        let json_line = serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "deref which would be done by auto-deref",
                "level": "warning",
                "spans": [
                    {
                        "file_name": "crates/core/src/library/db/mod.rs",
                        "line_start": 895,
                        "column_start": 32,
                        "line_end": 895,
                        "column_end": 36,
                        "text": "    let y: &str = &x;"
                    }
                ],
                "code": {
                    "code": "clippy::ptr_arg",
                    "explanation": "..."
                }
            }
        })
        .to_string();

        let result = json_to_short(&json_line);

        assert_eq!(
            result,
            "crates/core/src/library/db/mod.rs:895:32: warning: deref which would be done by auto-deref [clippy::ptr_arg]"
        );
    }

    #[test]
    fn json_to_short_handles_missing_code_field() {
        let json_line = serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "unused variable: `x`",
                "level": "warning",
                "spans": [
                    {
                        "file_name": "src/lib.rs",
                        "line_start": 10,
                        "column_start": 5,
                        "line_end": 10,
                        "column_end": 6,
                        "text": "let x = 1;"
                    }
                ]
            }
        })
        .to_string();

        let result = json_to_short(&json_line);

        assert_eq!(result, "src/lib.rs:10:5: warning: unused variable: `x`");
    }

    #[test]
    fn json_to_short_handles_error_level() {
        let json_line = serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "message": "expected `,`, found `{`",
                "level": "error",
                "spans": [
                    {
                        "file_name": "src/main.rs",
                        "line_start": 1,
                        "column_start": 1,
                        "line_end": 1,
                        "column_end": 1,
                        "text": "fn main() {"
                    }
                ],
                "code": {
                    "code": "E0789"
                }
            }
        })
        .to_string();

        let result = json_to_short(&json_line);

        assert_eq!(
            result,
            "src/main.rs:1:1: error: expected `,`, found `{` [E0789]"
        );
    }
}
