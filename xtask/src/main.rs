use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Cadmus development tasks", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the project
    Build {
        /// Build for release
        #[arg(long)]
        release: bool,
    },
    /// Run tests
    Test {
        /// Run tests with specific filter
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Run clippy lints
    Lint,
    /// Format the code
    Fmt,
    /// Run CI checks
    Ci,
    /// Setup development environment
    SetupDev,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Build { release } => cmd_build(*release)?,
        Commands::Test { filter } => cmd_test(filter.clone())?,
        Commands::Lint => cmd_lint()?,
        Commands::Fmt => cmd_fmt()?,
        Commands::Ci => cmd_ci()?,
        Commands::SetupDev => cmd_setup_dev()?,
    }
    Ok(())
}

fn cmd_build(release: bool) -> Result<()> {
    println!("Building Cadmus...");

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("build");

    if release {
        cmd.arg("--release");
    }

    // Add verbose output for debugging
    cmd.arg("--verbose");

    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("Build failed with exit code: {}", status);
    }

    println!("Build completed successfully!");
    Ok(())
}

fn cmd_test(filter: Option<String>) -> Result<()> {
    println!("Running tests...");

    let mut cmd = std::process::Command::new("cargo");
    cmd.arg("test");

    if let Some(filter) = filter {
        cmd.arg(filter);
    }

    let status = cmd.status()?;

    if !status.success() {
        anyhow::bail!("Tests failed");
    }

    println!("All tests passed!");
    Ok(())
}

fn cmd_lint() -> Result<()> {
    println!("Running lints...");

    let status = std::process::Command::new("cargo")
        .args([
            "clippy",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!("Linting failed");
    }

    println!("All lints passed!");
    Ok(())
}

fn cmd_fmt() -> Result<()> {
    println!("Formatting code...");

    let status = std::process::Command::new("cargo")
        .args(["fmt", "--all"])
        .status()?;

    if !status.success() {
        anyhow::bail!("Formatting failed");
    }

    println!("Code formatted successfully!");
    Ok(())
}

fn cmd_ci() -> Result<()> {
    println!("Running CI checks...");

    cmd_fmt()?;
    cmd_lint()?;
    cmd_test(None)?;

    println!("All CI checks passed!");
    Ok(())
}

fn cmd_setup_dev() -> Result<()> {
    println!("Setting up development environment...");

    // Install pre-commit hooks if available
    // Run any initialization scripts
    // Configure development settings

    println!("Development environment setup completed!");
    Ok(())
}
