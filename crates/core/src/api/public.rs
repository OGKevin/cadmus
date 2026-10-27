//! On-demand public companion listener (LAN, port 80 / ephemeral on emulator).

use super::{ApiSession, Command, JOIN_TIMEOUT, LEASE_NAME, serve};
use crate::device::wifi::{WifiLease, WifiSession, WifiSessionError};
use crate::device::{AppContext, EventOutcome};
use crate::runtime::Job;
use crate::settings::WifiMode;
use crate::view::{Event, Hub, ViewId};
use secrecy::SecretString;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[cfg(any(not(debug_assertions), test))]
use crate::device::DevicePaths;

/// Listen address for the public API.
///
/// The Kobo binds a dual-stack port 80, so IPv4 and IPv6 clients both reach it.
/// The emulator is not root, so it binds IPv4 port 0 and asks the kernel for a
/// free port. Tests bind port 0 because resuming needs a port an unprivileged
/// process may take.
pub(super) const BIND: SocketAddr = if cfg!(test) {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)
} else if cfg!(feature = "emulator") {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
} else {
    SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 80)
};

/// Why the public listener did not stay up.
#[derive(Debug, Error)]
pub(super) enum StartError {
    /// Wi-Fi mode is Off, so no lease was taken.
    #[error("wifi is off")]
    WifiOff,
    /// The lease could not be acquired.
    #[error("wifi lease failed")]
    Wifi(#[source] WifiSessionError),
    /// The listen port is not available.
    #[error("bind failed")]
    Bind(#[source] std::io::Error),
    /// The OS random source refused to produce a token.
    #[error("os randomness unavailable: {0}")]
    Rng(getrandom::Error),
    /// A stop or suspend superseded the start before it finished.
    #[error("start cancelled")]
    Cancelled,
}

/// Companion Wi-Fi lease and bound port handed back to the main loop.
pub(super) struct BoundListener {
    pub(super) addr: SocketAddr,
    pub(super) lease: WifiLease,
    pub(super) listener: TcpListener,
}

/// Listener start that has not reported back yet.
///
/// The job takes the Wi-Fi lease and binds the port. It exists only between
/// [`turn_on`] and the [`Command::ListenerBound`] command; dropping it cancels
/// the job, so a stop or suspend never races the start.
pub(super) struct PendingStart {
    pub(super) job: Job<()>,
}

/// Public listener job, companion Wi-Fi lease, optional mDNS, and site locale.
pub(super) struct Running {
    pub(super) job: Job<()>,
    pub(super) lease: WifiLease,
    pub(super) mdns: Option<crate::mdns::Registration>,
    #[cfg(test)]
    pub(super) local_addr: SocketAddr,
    pub(super) locale: cadmus_api_proto::cadmus::api::shared::v1::Locale,
}

/// Start or stop the public companion listener from the main-menu switch.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn toggle(hub: &Hub, context: &mut AppContext) -> EventOutcome {
    if context.api.is_on() {
        turn_off(context).await;
        return EventOutcome::Handled;
    }
    if let Err(error) = turn_on(hub, context, BIND, true).await {
        tracing::warn!(error = %error, "public api did not start");
    }
    EventOutcome::Handled
}

/// Start a job that takes the companion Wi-Fi lease and binds the listener.
///
/// On Kobo the lease only completes once the main-loop `NetUp` handler runs the
/// Wi-Fi bring-up, so awaiting it here would park the main loop for the whole
/// acquire timeout. The job reports back through [`Command::ListenerBound`],
/// and the loop finishes the start in [`finish_start`].
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(addr = %addr, fresh_token = fresh_token)))]
pub(super) async fn turn_on(
    hub: &Hub,
    context: &mut AppContext,
    addr: SocketAddr,
    fresh_token: bool,
) -> Result<(), StartError> {
    if context.api.pending_start.is_some() {
        return Ok(());
    }
    if context.settings.wifi == WifiMode::Off {
        return Err(StartError::WifiOff);
    }
    if fresh_token || context.api.token.is_none() {
        context.api.token = Some(SecretString::from(new_token()?));
        context.api.auth_failures.store(0, Ordering::Relaxed);
    }
    let generation = context.api.begin_start();
    let tx = context.api.sender();
    let hub_for_job = hub.clone();
    let session = context.wifi_session.clone();
    let job = Job::spawn(move |cancel| async move {
        let result = bind_and_lease(session, addr, cancel).await;
        if tx
            .send(Command::ListenerBound { generation, result })
            .is_ok()
        {
            hub_for_job.send(Event::Api.into()).ok();
        }
    });
    context.api.pending_start = Some(PendingStart { job });
    Ok(())
}

/// Take the lease and bind the port away from the main loop.
async fn bind_and_lease(
    session: std::sync::Arc<WifiSession>,
    addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<BoundListener, StartError> {
    let lease = tokio::select! {
        _ = cancel.cancelled() => return Err(StartError::Cancelled),
        result = session.acquire(LEASE_NAME) => match result {
            Ok(lease) => lease,
            Err(WifiSessionError::ModeOff) => return Err(StartError::WifiOff),
            Err(error) => return Err(StartError::Wifi(error)),
        },
    };
    let listener = serve::bind(addr).await.map_err(StartError::Bind)?;
    Ok(BoundListener {
        addr,
        lease,
        listener,
    })
}

/// Finish a start reported by [`Command::ListenerBound`].
///
/// A start that a stop or suspend superseded is ignored, and its lease and
/// listener are dropped with the message.
pub(super) async fn finish_start(
    hub: &Hub,
    context: &mut AppContext,
    generation: u64,
    result: Result<BoundListener, StartError>,
) {
    if context.api.start_generation != generation {
        return;
    }
    context.api.pending_start = None;
    let bound = match result {
        Ok(bound) => bound,
        Err(error) => {
            context.api.token = None;
            context.api.auth_failures.store(0, Ordering::Relaxed);
            tracing::warn!(error = %error, "public api did not start");
            hub.send((Event::Close(ViewId::CompanionDialog)).into())
                .ok();
            return;
        }
    };
    if let Err(error) = complete_start(hub, context, bound).await {
        context.api.token = None;
        context.api.auth_failures.store(0, Ordering::Relaxed);
        tracing::warn!(error = %error, "public api did not start");
        hub.send((Event::Close(ViewId::CompanionDialog)).into())
            .ok();
    }
}

/// URL the on-device QR panel encodes: the companion page the listener serves.
///
/// Mirrors [`complete_start`]: the site locale is resolved against the site
/// root, so the panel never advertises a page the listener cannot serve.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn companion_url(context: &AppContext) -> Option<String> {
    let root = site_root(context);
    let locale = serve::site_locale(&root, context.settings.locale.as_ref()).await;
    Some(crate::view::companion::companion_url(
        &context.mdns.hostname(),
        locale,
    ))
}

/// Register mDNS, serve the site, and store the running listener.
async fn complete_start(
    hub: &Hub,
    context: &mut AppContext,
    bound: BoundListener,
) -> Result<(), StartError> {
    let BoundListener {
        addr,
        lease,
        listener,
    } = bound;
    let local_addr = listener.local_addr().map_err(StartError::Bind)?;
    let token = context
        .api
        .token
        .clone()
        .ok_or_else(|| StartError::Bind(std::io::Error::other("missing token")))?;
    let root = site_root(context);
    let locale = serve::site_locale(&root, context.settings.locale.as_ref()).await;
    let ip = device_ip(context).await;
    let tx = context.api.sender();
    let hub_for_job = hub.clone();
    let inhibitor = std::sync::Arc::clone(&context.inhibitor);
    let path = format!("/{}/companion/", serve::locale_dir(locale));
    context.mdns.start();
    let mdns = if addr.ip().is_unspecified() {
        match crate::mdns::Subsystem::parse("companion") {
            Ok(subsystem) => context.mdns.register(crate::mdns::Service {
                ty: crate::mdns::ServiceType::tcp(subsystem),
                instance: crate::mdns::Instance::Hostname,
                port: local_addr.port(),
                txt: crate::mdns::TxtRecords::from_pairs([
                    ("vers".into(), "1".into()),
                    ("path".into(), path),
                ]),
                ip,
            }),
            Err(error) => {
                tracing::warn!(error = %error, "invalid mdns subsystem for the public api");
                None
            }
        }
    } else {
        None
    };
    let file_root = super::file_root(context);
    let auth_failures = std::sync::Arc::clone(&context.api.auth_failures);
    let job = Job::spawn(move |cancel| async move {
        serve::listen(
            cancel,
            listener,
            serve::ListenConfig {
                token: Some(token),
                site_root: Some(root),
                scope: serve::Scope::Public,
                file_root,
                auth_failures,
            },
            tx,
            hub_for_job,
            inhibitor,
        )
        .await;
    });
    context.api.public = Some(Running {
        job,
        lease,
        mdns,
        #[cfg(test)]
        local_addr,
        locale,
    });
    tracing::info!(
        port = local_addr.port(),
        locale = serve::locale_dir(locale),
        "public api listening"
    );
    Ok(())
}

/// Stop the public listener and forget the token.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn turn_off(context: &mut AppContext) {
    context.api.token = None;
    stop(&mut context.api).await;
    context.api.auth_failures.store(0, Ordering::Relaxed);
}

/// Close the public listener and drop the companion Wi-Fi lease. The token stays for resume.
///
/// The job is joined so the listen port is free before the device sleeps.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn suspend(context: &mut AppContext) {
    if context.api.token.is_none() {
        return;
    }
    stop(&mut context.api).await;
    tracing::info!("public api listener closed for suspend");
}

/// Rebind the public listener after suspend, keeping the same token.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn resume(hub: &Hub, context: &mut AppContext) -> EventOutcome {
    if context.api.token.is_none() || context.api.public.is_some() {
        return EventOutcome::Handled;
    }
    if let Err(error) = turn_on(hub, context, BIND, false).await {
        context.api.token = None;
        context.api.auth_failures.store(0, Ordering::Relaxed);
        tracing::warn!(error = %error, "public api did not resume");
    }
    EventOutcome::Handled
}

/// Cancel the public job, join it, then drop the companion Wi-Fi lease and
/// mDNS record. Cancelling first frees the port without waiting out the join
/// deadline, so suspend and shutdown never stall the main loop.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn stop(session: &mut ApiSession) {
    if let Some(pending) = session.take_pending_start() {
        pending.job.cancel();
        let _ = pending.job.join(JOIN_TIMEOUT).await;
    }
    if let Some(running) = session.public.take() {
        let Running {
            job, lease, mdns, ..
        } = running;
        job.cancel();
        let _ = job.join(JOIN_TIMEOUT).await;
        drop(mdns);
        drop(lease);
    }
}

/// Site directory for this process.
///
/// A debug build reads `website/out` in the workspace. Release builds and
/// tests read `website/` in the install directory.
#[cfg(all(debug_assertions, not(test)))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn site_root(_context: &AppContext) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../website/out")
}

#[cfg(any(not(debug_assertions), test))]
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn site_root(context: &AppContext) -> std::path::PathBuf {
    context.device.install_path("website")
}

/// Six decimal digits the user types into the companion client.
///
/// The value is the bearer token for the public listener, drawn from the OS
/// CSPRNG so it cannot be predicted from a seed. The auth failure limit closes
/// the guessing window that a six-digit space would otherwise leave open.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn new_token() -> Result<String, StartError> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(StartError::Rng)?;
    Ok(format!("{:06}", u32::from_le_bytes(bytes) % 1_000_000))
}

/// Address to put in the mDNS A record, if Wi-Fi has one.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn device_ip(context: &AppContext) -> Option<IpAddr> {
    context
        .wifi_session
        .wifi_manager()
        .network_info()
        .await
        .ok()
        .flatten()
        .map(|info| info.ip)
}

#[cfg(test)]
mod tests {
    use super::new_token;

    #[test]
    fn tokens_are_six_decimal_digits() {
        for _ in 0..8 {
            let token = new_token().expect("os randomness");
            assert_eq!(token.len(), 6);
            assert!(token.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }
}
