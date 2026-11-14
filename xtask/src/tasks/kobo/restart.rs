//! `cadmuscli kobo restart` — restart the on-device Cadmus process.
//!
//! The remote script signals only the Cadmus process whose working directory
//! is [`INSTALL_DIR`], so a second install running from elsewhere is left
//! alone. `SIGUSR1` makes Cadmus write `/tmp/restart`; `cadmus.sh` then
//! relaunches it. The script polls for a new pid for up to 30 seconds.

use anyhow::{Result, bail};
use russh::client::Handle;

use super::INSTALL_DIR;
use super::ssh::{self, ExecOutput, Handler};

/// The remote process was not running from the test install.
const EXIT_NOT_RUNNING: u32 = 1;
/// `SIGUSR1` was sent but no new pid appeared within the poll window.
const EXIT_NOT_RESTARTED: u32 = 2;

/// Remote restart script, kept as a standalone file so shellcheck and shfmt
/// lint it like every other shell script in the repository. The install dir is
/// passed in the environment; the exit codes are the constants above.
const RESTART_SCRIPT: &str = include_str!("restart.sh");

/// Builds the remote restart script with the install dir exported.
pub(crate) fn script() -> String {
    format!("export CADMUS_INSTALL_DIR={INSTALL_DIR}\n{RESTART_SCRIPT}")
}

/// Maps the remote exit status onto the user-facing outcome.
fn outcome(host: &str, output: &ExecOutput) -> Result<String> {
    let body = format!("{}{}", output.stdout, output.stderr);
    let body = body.trim();

    match output.exit_status {
        Some(0) => Ok(format!("Restart on {host}:\n{body}")),
        Some(EXIT_NOT_RUNNING) => {
            bail!("Cadmus is not running on {host}. Start it from the device, then try again.")
        }
        Some(EXIT_NOT_RESTARTED) => bail!("Cadmus did not restart on {host}:\n{body}"),
        Some(status) => bail!("the restart script on {host} exited with {status}\n{body}"),
        None => bail!(
            "ssh to {host} dropped before the restart result arrived\n{body}\n\
             Cadmus may already be restarting. Check the device before re-running \
             `cadmuscli kobo restart`, since another run sends SIGUSR1 again."
        ),
    }
}

/// Restarts Cadmus on the device.
pub(crate) async fn run(args: super::RestartArgs) -> Result<()> {
    let settings = ssh::resolve(&args.ssh).await?;
    let mut handle = ssh::connect(&settings).await?;
    restart(&mut handle, settings.host()).await
}

/// Restarts Cadmus over an existing authenticated connection.
pub(crate) async fn restart(handle: &mut Handle<Handler>, host: &str) -> Result<()> {
    let output = ssh::exec(handle, &script()).await?;
    println!("{}", outcome(host, &output)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::*;

    fn output(exit_status: Option<u32>, stdout: &str, stderr: &str) -> ExecOutput {
        ExecOutput {
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            exit_status,
        }
    }

    fn write_stub(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn run_script(dir: &Path) -> std::process::Output {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script())
            .env("PATH", dir)
            .env("STUB_DIR", dir)
            .env("CADMUS_INSTALL_DIR", INSTALL_DIR)
            .output()
            .expect("failed to run the restart script")
    }

    fn text(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr)
    }

    #[test]
    fn script_is_valid_shell() {
        let mut child = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(script().as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn script_restarts_a_process_in_the_test_install() {
        let dir = tempfile::tempdir().unwrap();
        write_stub(
            dir.path(),
            "pidof",
            r#"if [ -f "$STUB_DIR/killed" ]; then echo 200; else echo 100; fi"#,
        );
        write_stub(dir.path(), "readlink", &format!("echo {INSTALL_DIR}"));
        write_stub(dir.path(), "env", r#"exec "$@""#);
        write_stub(dir.path(), "kill", r#": > "$STUB_DIR/killed""#);
        write_stub(dir.path(), "sleep", ":");

        let output = run_script(dir.path());
        let text = text(&output);

        assert!(output.status.success(), "{text}");
        assert!(text.contains("cadmus pid: 100 -> 200"), "{text}");
        assert!(text.contains("RESULT: restarted"), "{text}");
    }

    #[test]
    fn script_ignores_a_process_in_another_install() {
        let dir = tempfile::tempdir().unwrap();
        write_stub(dir.path(), "pidof", "echo 100");
        write_stub(dir.path(), "readlink", "echo /mnt/onboard/.adds/cadmus");

        let output = run_script(dir.path());
        let text = text(&output);

        assert_eq!(
            output.status.code(),
            Some(EXIT_NOT_RUNNING as i32),
            "{text}"
        );
        assert!(text.contains("cadmus is not running"), "{text}");
    }

    #[test]
    fn script_fails_when_the_pid_does_not_change() {
        let dir = tempfile::tempdir().unwrap();
        write_stub(dir.path(), "pidof", "echo 100");
        write_stub(dir.path(), "readlink", &format!("echo {INSTALL_DIR}"));
        write_stub(dir.path(), "env", r#"exec "$@""#);
        write_stub(dir.path(), "kill", ":");
        write_stub(dir.path(), "sleep", ":");

        let output = run_script(dir.path());
        let text = text(&output);

        assert_eq!(
            output.status.code(),
            Some(EXIT_NOT_RESTARTED as i32),
            "{text}"
        );
        assert!(text.contains("cadmus pid: 100 -> 100"), "{text}");
        assert!(text.contains("RESULT: cadmus did not restart"), "{text}");
    }

    #[test]
    fn success_reports_the_pid_transition() {
        let message = outcome(
            "kobo",
            &output(Some(0), "cadmus pid: 100 -> 200\nRESULT: restarted\n", ""),
        )
        .unwrap();

        assert!(message.contains("Restart on kobo"));
        assert!(message.contains("100 -> 200"));
    }

    #[test]
    fn not_running_is_explained() {
        let error = outcome("kobo", &output(Some(EXIT_NOT_RUNNING), "", "")).unwrap_err();
        assert!(error.to_string().contains("not running on kobo"), "{error}");
    }

    #[test]
    fn not_restarted_includes_the_remote_output() {
        let error = outcome(
            "kobo",
            &output(Some(EXIT_NOT_RESTARTED), "cadmus pid: 100 -> none\n", ""),
        )
        .unwrap_err();

        assert!(error.to_string().contains("did not restart"), "{error}");
        assert!(error.to_string().contains("100 -> none"), "{error}");
    }

    #[test]
    fn unexpected_exit_status_is_reported() {
        let error = outcome("kobo", &output(Some(255), "", "connection closed\n")).unwrap_err();

        assert!(error.to_string().contains("exited with 255"), "{error}");
        assert!(error.to_string().contains("connection closed"), "{error}");
    }

    #[test]
    fn missing_exit_status_warns_about_a_second_signal() {
        let error = outcome("kobo", &output(None, "", "")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("dropped before the restart result"),
            "{error}"
        );
        assert!(error.to_string().contains("SIGUSR1"), "{error}");
    }
}
