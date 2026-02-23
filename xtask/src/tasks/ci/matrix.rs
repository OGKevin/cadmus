//! `cargo xtask ci matrix` — emit GitHub Actions dynamic matrix JSON.
//!
//! Scans the workspace `Cargo.toml` files for feature flags and writes two
//! outputs to `$GITHUB_OUTPUT`:
//!
//! - `matrix` — feature × OS entries (for the `clippy` job)
//! - `test-matrix` — feature entries for ubuntu test jobs
//!
//! ```text
//! matrix={"include":[{"label":"default","features":"","os":"ubuntu-latest"},…]}
//! test-matrix={"include":[{"label":"default","features":"","os":"ubuntu-latest"},…]}
//! ```
//!
//! ## Usage in a workflow
//!
//! ```yaml
//! generate-matrix:
//!   outputs:
//!     matrix: ${{ steps.matrix.outputs.matrix }}
//!     test-matrix: ${{ steps.matrix.outputs.test-matrix }}
//!   steps:
//!     - id: matrix
//!       run: cargo xtask ci matrix
//!
//! clippy:
//!   needs: generate-matrix
//!   strategy:
//!     matrix:
//!       include: ${{ fromJson(needs.generate-matrix.outputs.matrix).include }}
//!
//! test:
//!   needs: generate-matrix
//!   strategy:
//!     matrix:
//!       include: ${{ fromJson(needs.generate-matrix.outputs.test-matrix).include }}
//! ```

use std::io::Write;

use anyhow::{Context, Result};

use crate::tasks::util::{matrix, workspace};

/// Generates the feature matrix and writes both outputs to `$GITHUB_OUTPUT`.
///
/// # Errors
///
/// Returns an error if the workspace cannot be scanned, JSON serialisation
/// fails, or `$GITHUB_OUTPUT` cannot be written.
pub fn run() -> Result<()> {
    let root = workspace::root()?;
    let entries = matrix::scan(&root, matrix::CI_CLIPPY_OS)?;

    let clippy_json = matrix::to_github_matrix_json(&entries)?;
    write_github_output("matrix", &clippy_json)?;

    let test_json = matrix::to_github_test_matrix_json(&entries)?;
    write_github_output("test-matrix", &test_json)
}

fn write_github_output(key: &str, value: &str) -> Result<()> {
    match std::env::var("GITHUB_OUTPUT") {
        Ok(path) => {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .with_context(|| format!("failed to open GITHUB_OUTPUT at {path}"))?;
            writeln!(file, "{key}={value}")
                .with_context(|| format!("failed to write to GITHUB_OUTPUT at {path}"))
        }
        Err(_) => {
            println!("{value}");
            Ok(())
        }
    }
}
