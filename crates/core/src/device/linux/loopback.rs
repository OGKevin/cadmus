//! Loopback interface control.
//!
//! Kobo boots with `lo` down, and [`mdns_sd`](https://crates.io/crates/mdns-sd)
//! binds its daemon signal socket to `127.0.0.1`. Discovery therefore fails
//! with `EADDRNOTAVAIL` until the interface is up. The Kobo lifecycle raises it
//! in `on_startup` and restores the boot state in `on_shutdown`.

use tokio::process::Command;

const INTERFACE: &str = "lo";

/// Brings the loopback interface up.
pub(crate) async fn bring_up() {
    set(true).await;
}

/// Brings the loopback interface down, matching the Kobo boot state.
pub(crate) async fn bring_down() {
    set(false).await;
}

/// Applies the requested state.
///
/// Tests run on the host, where the loopback interface is already managed by
/// the OS, so they never spawn the command.
async fn set(up: bool) {
    if cfg!(test) {
        return;
    }
    match command(up).status().await {
        Ok(status) if status.success() => {}
        Ok(status) => {
            tracing::warn!(up, status = %status, "loopback command failed");
        }
        Err(error) => {
            tracing::warn!(up, error = %error, "loopback command failed");
        }
    }
}

fn command(up: bool) -> Command {
    let mut command = Command::new("ifconfig");
    command.arg(INTERFACE).arg(if up { "up" } else { "down" });
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(up: bool) -> Vec<String> {
        command(up)
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn command_toggles_loopback() {
        assert_eq!(command(true).as_std().get_program(), "ifconfig");
        assert_eq!(args(true), ["lo", "up"]);
        assert_eq!(args(false), ["lo", "down"]);
    }
}
