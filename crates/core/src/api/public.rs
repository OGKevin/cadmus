//! On-demand public companion listener (LAN, port 80 / ephemeral on emulator).

use super::{ApiSession, JOIN_TIMEOUT, LEASE_NAME, serve};
use crate::device::wifi::{WifiLease, WifiSessionError};
use crate::device::{AppContext, EventOutcome};
use crate::runtime::Job;
use crate::view::Hub;
use rand_core::Rng;
use rand_xoshiro::Xoroshiro128Plus;
use secrecy::SecretString;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use thiserror::Error;

#[cfg(any(not(debug_assertions), test))]
use crate::device::DevicePaths;

/// Listen address for the public API.
///
/// The Kobo binds a dual-stack port 80, so IPv4 and IPv6 clients both reach it.
/// The emulator is not root, so it binds IPv4 port 0 and asks the kernel for a
/// free port. Tests bind port 0 because resuming needs a port an unprivileged
/// process may take.
pub(super) const BIND: SocketAddr = if cfg!(feature = "emulator") {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
} else {
    SocketAddr::new(
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        if cfg!(test) { 0 } else { 80 },
    )
};

/// Why the public listener did not stay up.
#[derive(Debug, Error)]
pub(crate) enum StartError {
    /// Wi-Fi mode is Off, so no lease was taken.
    #[error("wifi is off")]
    WifiOff,
    /// The lease could not be acquired.
    #[error("wifi lease failed")]
    Wifi(#[source] WifiSessionError),
    /// The listen port is not available.
    #[error("bind failed")]
    Bind(#[source] std::io::Error),
}

/// Public listener job, companion Wi-Fi lease, optional mDNS, and site locale.
pub(super) struct Running {
    pub job: Job<()>,
    pub lease: WifiLease,
    pub mdns: Option<crate::mdns::Registration>,
    #[cfg(test)]
    pub local_addr: SocketAddr,
    pub locale: cadmus_api_proto::cadmus::api::shared::v1::Locale,
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

/// Bind the public listener, take the companion Wi-Fi lease, and serve the site.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(addr = %addr, fresh_token)))]
pub(super) async fn turn_on(
    hub: &Hub,
    context: &mut AppContext,
    addr: SocketAddr,
    fresh_token: bool,
) -> Result<(), StartError> {
    let lease = match context.wifi_session.acquire(LEASE_NAME).await {
        Ok(lease) => lease,
        Err(WifiSessionError::ModeOff) => return Err(StartError::WifiOff),
        Err(error) => return Err(StartError::Wifi(error)),
    };
    let listener = match serve::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => return Err(StartError::Bind(error)),
    };
    let local_addr = listener.local_addr().map_err(StartError::Bind)?;
    if fresh_token || context.api.token.is_none() {
        context.api.token = Some(SecretString::from(new_token(&mut context.rng)));
    }
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
        context.mdns.register(crate::mdns::Service {
            ty: crate::mdns::ServiceType::tcp(
                crate::mdns::Subsystem::parse("companion").expect("companion is a valid subsystem"),
            ),
            instance: crate::mdns::Instance::Hostname,
            port: local_addr.port(),
            txt: crate::mdns::TxtRecords::from_pairs([
                ("vers".into(), "1".into()),
                ("path".into(), path),
            ]),
            ip,
        })
    } else {
        None
    };
    let file_root = super::file_root(context);
    let job = Job::spawn(move |cancel| async move {
        serve::listen(
            cancel,
            listener,
            serve::ListenConfig {
                token: Some(token),
                site_root: Some(root),
                scope: serve::Scope::Public,
                file_root,
            },
            tx,
            hub_for_job,
            inhibitor,
        )
        .await;
    });
    context.api.suspended = false;
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
        locale = context
            .api
            .public
            .as_ref()
            .map(|r| serve::locale_dir(r.locale))
            .unwrap_or_default(),
        "public api listening"
    );
    Ok(())
}

/// Stop the public listener and forget the token.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(crate) async fn turn_off(context: &mut AppContext) {
    context.api.token = None;
    context.api.suspended = false;
    stop(&mut context.api).await;
}

/// Close the public listener and drop the companion Wi-Fi lease. The token stays for resume.
///
/// The job is joined so the listen port is free before the device sleeps.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(crate) async fn suspend(context: &mut AppContext) {
    if context.api.token.is_none() {
        return;
    }
    context.api.suspended = true;
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
        context.api.suspended = false;
        tracing::warn!(error = %error, "public api did not resume");
    }
    EventOutcome::Handled
}

/// Join the public job and drop the companion Wi-Fi lease and mDNS record.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn stop(session: &mut ApiSession) {
    if let Some(running) = session.public.take() {
        let Running {
            job, lease, mdns, ..
        } = running;
        drop(mdns);
        drop(lease);
        let _ = job.join(JOIN_TIMEOUT).await;
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

/// Four decimal digits, in memory only.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn new_token(rng: &mut Xoroshiro128Plus) -> String {
    format!("{:04}", rng.next_u32() % 10_000)
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
