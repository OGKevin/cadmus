//! Connect RPC dispatch shared by the public and private HTTP listeners.
//!
//! Handlers never touch [`crate::device::AppContext`]. They enqueue a [`Command`] on the API
//! channel and wake the main loop with [`crate::view::Event::Api`]. The loop
//! drains commands and replies on a oneshot. That path is the same for loopback
//! and LAN listeners.
//!
//! While a handler waits for the reply, it holds a [`Kind::SoftSuspend`] lease
//! and attaches a matching lease to the hub message so autosleep does not run
//! before the main loop acquires its own overlap lease.

use super::{COMMAND_TIMEOUT, Command};
use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
use crate::view::{Event, Hub, HubMessage};
use connectrpc::{ConnectError, ErrorCode};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Public Connect handler: the `cadmus.api.shared.v1` services.
///
/// Private-only services are implemented on
/// [`super::internal::InternalRpc`], which wraps this type, so a private
/// handler can reuse the shared plumbing without re-implementing the public
/// services.
#[derive(Clone)]
pub(super) struct PublicRpc {
    pub tx: mpsc::UnboundedSender<Command>,
    pub hub: Hub,
    pub inhibitor: Arc<Inhibitor>,
    /// Uppermost directory `FileOps` may read. `None` is unrestricted.
    pub file_root: Option<Arc<PathBuf>>,
}

/// Queue `make` on the main loop and wait for the oneshot reply.
///
/// Sends [`Event::Api`] with a soft-suspend hub lease and keeps a worker lease
/// until the reply arrives or [`COMMAND_TIMEOUT`] elapses.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn ask<T, F>(
    tx: &mpsc::UnboundedSender<Command>,
    hub: &Hub,
    inhibitor: &Inhibitor,
    make: F,
) -> Result<T, ConnectError>
where
    F: FnOnce(tokio::sync::oneshot::Sender<Result<T, ConnectError>>) -> Command,
{
    let _soft_suspend = match inhibitor.acquire(Kind::SoftSuspend, SoftSuspendName::Api) {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(
                error = %error,
                soft_suspend_lease = %SoftSuspendName::Api,
                "failed to acquire soft-suspend lease for api request"
            );
            return Err(ConnectError::unavailable("api stopped"));
        }
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(make(reply_tx))
        .map_err(|_| ConnectError::unavailable("api stopped"))?;
    let message = HubMessage::try_with_soft_suspend(inhibitor, SoftSuspendName::Api, Event::Api);
    if hub.send(message).is_err() {
        return Err(ConnectError::unavailable("api stopped"));
    }
    match tokio::time::timeout(COMMAND_TIMEOUT, reply_rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(ConnectError::unavailable("api dropped the reply")),
        Err(_) => Err(ConnectError::new(
            ErrorCode::DeadlineExceeded,
            "api did not answer",
        )),
    }
}
