//! # xtask — Cadmus build automation
//!
//! Centralises every build, test, lint, documentation, and release task for
//! the Cadmus project as typed, testable Rust code that behaves identically
//! in a local devenv shell and in GitHub Actions CI.
//!
//! ## Usage
//!
//! ```text
//! cadmuscli <COMMAND> [OPTIONS]
//! ```
//!
//! `cargo xtask` still works via the cargo alias in `.cargo/config.toml`
//! (a debug `cargo run` of this package).
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
//! | [`sqlx_prepare`](tasks::sqlx_prepare) | Rebuild `.sqlx/` from a temporary database |
//! | [`list-devices`](tasks::list_devices) | List Cadmus devices on the LAN via mDNS |
//! | [`ci`](tasks::ci) | CI-specific setup tasks (e.g. `install-doc-tools`) |
//! | [`completions`](tasks::completions) | Emit shell completion scripts |
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
    clippy::ClippyArgs, completions::CompletionsArgs, dist::DistArgs, docs::DocsArgs, fmt::FmtArgs,
    install_importer::InstallImporterArgs, list_devices::ListDevicesArgs,
    run_emulator::RunEmulatorArgs, setup::SetupArgs, test::TestArgs,
};

/// Cadmus build automation.
///
/// Run `cadmuscli <COMMAND> --help` for per-command options.
#[derive(Debug, Parser)]
#[command(name = "cadmuscli", about = "Cadmus build automation")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Run benchmarks with the bench feature enabled.
    Bench(BenchArgs),
    /// Cross-compile Cadmus for Kobo devices (Linux & macOS).
    BuildKobo(BuildKoboArgs),
    /// Package a KoboRoot.tgz ready for device installation.
    Bundle(BundleArgs),
    /// CI-specific setup tasks (install-doc-tools, etc.).
    Ci(CiArgs),
    /// Run cargo clippy across the full feature matrix.
    Clippy(ClippyArgs),
    /// Emit shell completion scripts.
    Completions(CompletionsArgs),
    /// Assemble the Kobo distribution directory from build outputs.
    Dist(DistArgs),
    /// Build the full documentation website (mdBook + cargo doc + Next.js).
    Docs(DocsArgs),
    /// Download static asset directories from the latest GitHub release.
    DownloadAssets,
    /// Download bundled font files into the workspace `fonts/` directory.
    DownloadFonts,
    /// Check (or apply) rustfmt formatting across the workspace.
    Fmt(FmtArgs),
    /// Install the Cadmus importer crate.
    InstallImporter(InstallImporterArgs),
    /// List Cadmus devices advertised on the LAN via mDNS.
    ListDevices(ListDevicesArgs),
    /// Run the Cadmus emulator.
    RunEmulator(RunEmulatorArgs),
    /// Build thirdparty deps (SQLite) that must exist before `cargo build`.
    Setup(SetupArgs),
    /// Rebuild `.sqlx/` from a temporary database that contains both schemas.
    SqlxPrepare,
    /// Run cargo test across the full feature matrix.
    Test(TestArgs),
}

/// Run the xtask CLI with the given arguments.
///
/// This is the main entry point for the binary, but can also be called
/// from tests.
pub fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Bench(args) => tasks::bench::run(args),
        Command::BuildKobo(args) => tasks::build_kobo::run(args),
        Command::Bundle(args) => tasks::bundle::run(args),
        Command::Ci(args) => tasks::ci::run(args),
        Command::Clippy(args) => tasks::clippy::run(args),
        Command::Completions(args) => tasks::completions::run(args),
        Command::Dist(args) => tasks::dist::run(args),
        Command::Docs(args) => tasks::docs::run(args),
        Command::DownloadAssets => tasks::download_assets::run(),
        Command::DownloadFonts => tasks::download_fonts::run(),
        Command::Fmt(args) => tasks::fmt::run(args),
        Command::InstallImporter(args) => tasks::install_importer::run(args),
        Command::ListDevices(args) => tasks::list_devices::run(args),
        Command::RunEmulator(args) => tasks::run_emulator::run(args),
        Command::Setup(args) => tasks::setup::run(args),
        Command::SqlxPrepare => tasks::sqlx_prepare::run(),
        Command::Test(args) => tasks::test::run(args),
    }
}

#[cfg(test)]
mod cli_tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn top_level_subcommands_are_alphabetical() {
        let cmd = Cli::command();
        let names: Vec<String> = cmd
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
