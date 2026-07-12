//! # xtask — Cadmus build automation
//!
//! Centralises every build, test, lint, documentation, and release task for
//! the Cadmus project as typed, testable Rust code that behaves identically
//! in a local devenv shell and in GitHub Actions CI.
//!
//! ## Usage
//!
//! ```text
//! cargo xtask <COMMAND> [OPTIONS]
//! ```
//!
//! The `xtask` alias is configured in `.cargo/config.toml` so that
//! `cargo xtask` works from any directory inside the workspace.
//!
//! ## Commands
//!
//! | Command | Description |
//! |---------|-------------|
//! | [`fmt`](tasks::fmt) | Check (or apply) `rustfmt` formatting |
//! | [`clippy`](tasks::clippy) | Run `cargo clippy` across the feature matrix |
//! | [`test`](tasks::test) | Run `cargo test` across the feature matrix |
//! | [`bench`](tasks::bench) | Run benchmarks with the `bench` feature enabled |
//! | [`build-kobo`](tasks::build_kobo) | Cross-compile for Kobo (ARM, Linux & macOS) |
//! | [`run-emulator`](tasks::run_emulator) | Run the Cadmus emulator |
//! | [`install-importer`](tasks::install_importer) | Install the Cadmus importer crate |
//! | [`docs`](tasks::docs) | Build the full documentation website |
//! | [`download-assets`](tasks::download_assets) | Download static asset dirs from the latest release |
//! | [`download-fonts`](tasks::download_fonts) | Download bundled font files into `fonts/` |
//! | [`dist`](tasks::dist) | Assemble the Kobo distribution directory |
//! | [`bundle`](tasks::bundle) | Package a `KoboRoot.tgz` ready for device installation |
//! | [`setup`](tasks::setup) | Build thirdparty deps (SQLite) needed before `cargo build` |
//! | [`ci`](tasks::ci) | CI-specific setup tasks (e.g. `install-doc-tools`) |
//!
//! ## Design
//!
//! Each command lives in its own module under [`tasks`].  The `tasks::util::cmd`
//! helper wraps [`std::process::Command`] with consistent error reporting so
//! every task fails fast with a clear message.  The `tasks::util::http` module
//! provides pure-Rust download and archive helpers replacing `curl`, `wget`,
//! `tar`, and `sha256sum` subprocess calls.

pub mod tasks;

pub use anyhow::Result;
pub use clap::Parser;

pub use tasks::{
    bench::BenchArgs, build_kobo::BuildKoboArgs, bundle::BundleArgs, ci::CiArgs,
    clippy::ClippyArgs, dist::DistArgs, docs::DocsArgs, fmt::FmtArgs,
    install_importer::InstallImporterArgs, run_emulator::RunEmulatorArgs, setup::SetupArgs,
    test::TestArgs,
};

/// Cadmus build automation.
///
/// Run `cargo xtask <COMMAND> --help` for per-command options.
#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Cadmus build automation")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Check (or apply) rustfmt formatting across the workspace.
    Fmt(FmtArgs),
    /// Run cargo clippy across the full feature matrix.
    Clippy(ClippyArgs),
    /// Run cargo test across the full feature matrix.
    Test(TestArgs),
    /// Run benchmarks with the bench feature enabled.
    Bench(BenchArgs),
    /// Cross-compile Cadmus for Kobo devices (Linux & macOS).
    BuildKobo(BuildKoboArgs),
    /// Run the Cadmus emulator.
    RunEmulator(RunEmulatorArgs),
    /// Install the Cadmus importer crate.
    InstallImporter(InstallImporterArgs),
    /// Build the full documentation website (mdBook + cargo doc + Next.js).
    Docs(DocsArgs),
    /// Download static asset directories from the latest GitHub release.
    DownloadAssets,
    /// Download bundled font files into the workspace `fonts/` directory.
    DownloadFonts,
    /// Assemble the Kobo distribution directory from build outputs.
    Dist(DistArgs),
    /// Package a KoboRoot.tgz ready for device installation.
    Bundle(BundleArgs),
    /// Build thirdparty deps (SQLite) that must exist before `cargo build`.
    Setup(SetupArgs),
    /// CI-specific setup tasks (install-doc-tools, etc.).
    Ci(CiArgs),
}

/// Run the xtask CLI with the given arguments.
///
/// This is the main entry point for the binary, but can also be called
/// from tests.
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Fmt(args) => tasks::fmt::run(args),
        Command::Clippy(args) => tasks::clippy::run(args),
        Command::Test(args) => tasks::test::run(args),
        Command::Bench(args) => tasks::bench::run(args),
        Command::BuildKobo(args) => tasks::build_kobo::run(args),
        Command::RunEmulator(args) => tasks::run_emulator::run(args),
        Command::InstallImporter(args) => tasks::install_importer::run(args),
        Command::Docs(args) => tasks::docs::run(args),
        Command::DownloadAssets => tasks::download_assets::run(),
        Command::DownloadFonts => tasks::download_fonts::run(),
        Command::Dist(args) => tasks::dist::run(args),
        Command::Bundle(args) => tasks::bundle::run(args),
        Command::Setup(args) => tasks::setup::run(args),
        Command::Ci(args) => tasks::ci::run(args),
    }
}
