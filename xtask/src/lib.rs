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
//! | [`kobo`](tasks::kobo) | Deploy and restart Cadmus on a Kobo over SSH |
//! | [`run-emulator`](tasks::run_emulator) | Run the Cadmus emulator |
//! | [`install-importer`](tasks::install_importer) | Install the Cadmus importer crate |
//! | [`docs`](tasks::docs) | Build the full documentation website |
//! | [`download-assets`](tasks::download_assets) | Download static asset dirs from the latest release |
//! | [`download-fonts`](tasks::download_fonts) | Download bundled font files into `fonts/` |
//! | [`dist`](tasks::dist) | Assemble the Kobo distribution directory |
//! | [`bundle`](tasks::bundle) | Package a `KoboRoot.tgz` ready for device installation |
//! | [`setup`](tasks::setup) | Build thirdparty deps (SQLite) needed before `cargo build` |
//! | [`sqlx_prepare`](tasks::sqlx_prepare) | Rebuild `.sqlx/` from a temporary database |
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
    install_importer::InstallImporterArgs, kobo::KoboArgs, run_emulator::RunEmulatorArgs,
    setup::SetupArgs, test::TestArgs,
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
    /// Deploy and restart Cadmus on a Kobo over SSH.
    Kobo(KoboArgs),
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
pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Bench(args) => blocking(move || tasks::bench::run(args)).await?,
        Command::BuildKobo(args) => blocking(move || tasks::build_kobo::run(args)).await?,
        Command::Bundle(args) => blocking(move || tasks::bundle::run(args)).await?,
        Command::Ci(args) => blocking(move || tasks::ci::run(args)).await?,
        Command::Clippy(args) => blocking(move || tasks::clippy::run(args)).await?,
        Command::Completions(args) => blocking(move || tasks::completions::run(args)).await?,
        Command::Dist(args) => blocking(move || tasks::dist::run(args)).await?,
        Command::Docs(args) => blocking(move || tasks::docs::run(args)).await?,
        Command::DownloadAssets => blocking(tasks::download_assets::run).await?,
        Command::DownloadFonts => blocking(tasks::download_fonts::run).await?,
        Command::Fmt(args) => blocking(move || tasks::fmt::run(args)).await?,
        Command::InstallImporter(args) => {
            blocking(move || tasks::install_importer::run(args)).await?
        }
        Command::Kobo(args) => tasks::kobo::run(args).await?,
        Command::RunEmulator(args) => blocking(move || tasks::run_emulator::run(args)).await?,
        Command::Setup(args) => blocking(move || tasks::setup::run(args)).await?,
        Command::SqlxPrepare => blocking(tasks::sqlx_prepare::run).await?,
        Command::Test(args) => blocking(move || tasks::test::run(args)).await?,
    }

    Ok(())
}

/// Runs a blocking task on the blocking pool and propagates its result.
///
/// Every existing task shells out to `cargo` or similar and blocks until the
/// child exits, so they all go through here instead of the async worker pool.
pub(crate) async fn blocking<T, F>(task: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(task).await?
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
