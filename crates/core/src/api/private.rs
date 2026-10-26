//! Always-on loopback listener. Shares the HTTP accept loop with the public API.
//!
//! Serves every `cadmus.api.shared.v1` RPC plus `cadmus.api.internal.v1`; the
//! public listener serves only the shared ones.

use super::{ApiSession, JOIN_TIMEOUT, serve};
use crate::device::AppContext;
use crate::runtime::Job;
use crate::view::Hub;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Loopback listen address.
///
/// The port is fixed because this listener is never advertised: mDNS cannot
/// scope a record to loopback, so port 0 would leave on-device callers with no
/// way to find it. Tests bind port 0 so parallel suites do not collide.
const BIND: SocketAddr = SocketAddr::new(
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    if cfg!(test) { 0 } else { 8474 },
);

/// Start the private listener if it is not already running.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(crate) async fn start(hub: &Hub, context: &mut AppContext) {
    if context.api.private.is_some() {
        return;
    }
    let listener = match serve::bind(BIND).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(error = %error, "private api did not bind");
            return;
        }
    };
    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(error) => {
            tracing::warn!(error = %error, "private api address unavailable");
            return;
        }
    };
    let tx = context.api.sender();
    let hub_for_job = hub.clone();
    let inhibitor = std::sync::Arc::clone(&context.inhibitor);
    let file_root = super::file_root(context);
    let job = Job::spawn(move |cancel| async move {
        serve::listen(
            cancel,
            listener,
            serve::ListenConfig {
                token: None,
                site_root: None,
                scope: serve::Scope::Internal,
                file_root,
            },
            tx,
            hub_for_job,
            inhibitor,
        )
        .await;
    });
    context.api.private = Some(job);
    #[cfg(test)]
    {
        context.api.private_addr = Some(local_addr);
    }
    tracing::info!(port = local_addr.port(), "private api listening");
}

/// Join the private job.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn stop(session: &mut ApiSession) {
    if let Some(job) = session.private.take() {
        let _ = job.join(JOIN_TIMEOUT).await;
    }
    #[cfg(test)]
    {
        session.private_addr = None;
    }
}
