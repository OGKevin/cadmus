//! Command execution helpers.
//!
//! All tasks use [`run`] to execute external processes.  It prints the command
//! before running it (for CI log visibility) and converts non-zero exit codes
//! into descriptive [`anyhow::Error`] values so callers can use `?`.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};

/// Runs an external command, streaming its output to the terminal.
///
/// The command is printed before execution so CI logs show exactly what ran.
/// A non-zero exit status is converted to an error.
///
/// # Arguments
///
/// * `program` – The executable to run (looked up via `PATH`).
/// * `args` – Arguments passed to the program.
/// * `dir` – Working directory for the process.
/// * `env` – Additional environment variables (key-value pairs).
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or exits with a non-zero
/// status code.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use xtask_lib::tasks::util::cmd::run;
///
/// // Run `cargo fmt --check` in the workspace root.
/// run("cargo", &["fmt", "--check"], Path::new("."), &[])?;
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn run(program: &str, args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    let status = build_command(program, args, dir, env)
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;

    check_status(program, status)
}

/// Runs an external command and captures its stdout as a `String`.
///
/// Stderr is inherited (printed to the terminal).  The returned string has
/// leading/trailing whitespace trimmed.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned, exits with a non-zero
/// status, or produces non-UTF-8 output.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use xtask_lib::tasks::util::cmd::output;
///
/// let version = output("cargo", &["--version"], Path::new("."), &[])?;
/// assert!(version.starts_with("cargo "));
/// # Ok::<(), anyhow::Error>(())
/// ```
pub fn output(program: &str, args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Result<String> {
    let out = build_command(program, args, dir, env)
        .output()
        .with_context(|| format!("failed to spawn `{program}`"))?;

    check_status(program, out.status)?;

    let stdout = String::from_utf8(out.stdout)
        .with_context(|| format!("`{program}` produced non-UTF-8 output"))?;

    Ok(stdout.trim().to_owned())
}

fn build_command(program: &str, args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Command {
    let display_args = args.join(" ");
    println!("$ {program} {display_args}");

    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(dir);

    for (key, value) in env {
        cmd.env(OsStr::new(key), OsStr::new(value));
    }

    cmd
}

fn check_status(program: &str, status: ExitStatus) -> Result<()> {
    if status.success() {
        return Ok(());
    }

    match status.code() {
        Some(code) => bail!("`{program}` exited with status {code}"),
        None => bail!("`{program}` was terminated by a signal"),
    }
}
