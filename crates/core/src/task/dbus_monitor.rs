//! D-Bus system bus monitor for debugging.
//!
//! Subscribes to all signals on the system bus and debug-logs every event.
//! This is a diagnostic tool for investigating dhcpcd-dbus and
//! wpa_supplicant interactions on Kobo devices (CAD-18).

use futures_util::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::task::{BackgroundTask, TaskFuture, TaskId};

/// Monitors the system D-Bus and logs all signal events.
///
/// Connects to the system bus via zbus, subscribes to every signal,
/// and emits a `tracing::debug!` for each one. Intended as a
/// development/diagnostic aid — only compiled when both `test` and
/// `kobo` features are enabled.
pub struct DbusMonitorTask;

impl BackgroundTask for DbusMonitorTask {
    fn id(&self) -> TaskId {
        TaskId::DbusMonitor
    }

    fn run<'a>(
        &'a mut self,
        _hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    tracing::info!("shutdown requested");
                }
                result = monitor() => {
                    if let Err(e) = result {
                        tracing::error!(error = %e, "dbus monitor exited with error");
                    }
                }
            }
        })
    }
}

async fn monitor() -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::Connection::system().await?;
    tracing::info!("connected to system bus");

    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .build();

    let mut stream = zbus::MessageStream::for_match_rule(rule, &connection, Some(100)).await?;

    tracing::info!("subscribed to all signals");

    loop {
        tokio::select! {
            biased;

            msg = stream.next() => {
                let Some(msg) = msg else { break };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to read dbus message");
                        continue;
                    }
                };

                let body = msg.body();
                let body: Option<zbus::zvariant::Structure> = match body.deserialize() {
                    Ok(b) => Some(b),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to deserialize dbus message body");
                        None
                    }
                };

                let header = msg.header();
                tracing::debug!(
                    dbus_message = ?msg,
                    dbus_sender = ?header.sender(),
                    dbus_path = ?header.path(),
                    dbus_interface = ?header.interface(),
                    dbus_member = ?header.member(),
                    dbus_body = ?body,

                    "dbus signal"
                );
            }
        }
    }

    tracing::info!("dbus monitor stopped");
    Ok(())
}
