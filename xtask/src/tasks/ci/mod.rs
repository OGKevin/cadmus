//! `cargo xtask ci` — tasks that run exclusively in CI environments.
//!
//! These tasks handle CI-specific setup that would be awkward to express as
//! GitHub Actions YAML steps, such as installing and caching external tools
//! with version pinning.
//!
//! ## Subcommands
//!
//! | Subcommand | Description |
//! |------------|-------------|
//! | `install-doc-tools` | Install mdBook, mdbook-epub, mdbook-mermaid, and optionally Zola |
//! | `matrix` | Emit a GitHub Actions dynamic feature matrix JSON |
//! | `clippy-report` | Deduplicate clippy JSON artifacts and report via reviewdog |

pub mod clippy_report;
pub mod install_doc_tools;
pub mod matrix;

use anyhow::Result;
use clap::{Args, Subcommand};

use clippy_report::ClippyReportArgs;
use install_doc_tools::InstallDocToolsArgs;

/// Arguments for `cargo xtask ci`.
#[derive(Debug, Args)]
pub struct CiArgs {
    #[command(subcommand)]
    pub command: CiCommand,
}

/// Subcommands available under `cargo xtask ci`.
#[derive(Debug, Subcommand)]
pub enum CiCommand {
    /// Install and cache mdBook, mdbook-epub, mdbook-mermaid, and optionally Zola.
    ///
    /// Reads from the GitHub Actions cache (populated by `actions/cache` before
    /// this step) and installs any tools not already present.  Appends the
    /// install directory to `$GITHUB_PATH` so subsequent steps can invoke the
    /// tools directly.
    InstallDocTools(InstallDocToolsArgs),

    /// Scan workspace Cargo.toml files and emit a GitHub Actions dynamic matrix JSON.
    ///
    /// Writes `matrix=<json>` to `$GITHUB_OUTPUT` (or stdout when run locally).
    /// Consume the output with `fromJson(needs.<job>.outputs.matrix)`.
    Matrix,

    /// Deduplicate clippy JSON artifacts and post a single reviewdog report.
    ///
    /// Reads every `.json` file produced by `cargo xtask clippy --save-json`
    /// from `--artifacts-dir`, deduplicates diagnostics across feature labels,
    /// and pipes the unique set through `reviewdog` once using the
    /// `github-pr-review` reporter.
    ClippyReport(ClippyReportArgs),
}

/// Dispatches `cargo xtask ci <subcommand>`.
///
/// # Errors
///
/// Propagates any error returned by the selected subcommand.
pub fn run(args: CiArgs) -> Result<()> {
    match args.command {
        CiCommand::InstallDocTools(args) => install_doc_tools::run(args),
        CiCommand::Matrix => matrix::run(),
        CiCommand::ClippyReport(args) => clippy_report::run(args),
    }
}
