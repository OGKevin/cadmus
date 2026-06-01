//! Shell-out helpers used by the rest of `build_deps`.
//!
//! All build steps fork out to the host or cross toolchain (`make`,
//! `cmake`, `meson`, `ar`, `patch`, `readelf`, ...). The helpers in
//! this module wrap those invocations so that callers can write
//! straight-line code without manually wiring up status checks, error
//! contexts or working-directory handling.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};

/// Run `program` with `args` in `dir`, inheriting the parent
/// environment plus `env`. The invocation is echoed to stdout and any
/// non-zero exit is reported as an error.
pub fn run(program: &str, args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Result<()> {
    let status = build_command(program, args, dir, env)
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;

    check_status(program, status)
}

/// Run `program` with `args` in `dir` and return its trimmed stdout
/// as UTF-8. Non-zero exits and non-UTF-8 output are surfaced as
/// errors.
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
