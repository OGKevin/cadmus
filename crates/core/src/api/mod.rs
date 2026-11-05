//! Public and private Cadmus APIs.
//!
//! Shared command dispatch, settings catalog, file ops, and status live here.
//! The public listener is on-demand (Companion menu switch). The private
//! loopback listener starts with the process. Handlers do not touch
//! [`AppContext`]. They queue a command and wake the main loop, which replies
//! on a oneshot.

mod files;
mod handler;
mod internal;
mod private;
mod public;
mod serve;
mod settings;
mod status;

#[cfg(test)]
mod tests;

use crate::device::{AppContext, DeviceRuntime, EventOutcome};
use crate::runtime::Job;
use crate::view::{Bus, Event, Hub};
use secrecy::SecretString;
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(test)]
use std::net::SocketAddr;

#[cfg(test)]
pub(crate) use settings::toggle_value;

pub(crate) use cadmus_api_proto::cadmus::api::shared::v1 as proto;
#[cfg(test)]
pub(crate) use public::StartError;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const JOIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const LEASE_NAME: &str = "public-api";

/// Work a listener asks the main loop to do.
pub(crate) enum Command {
    /// Locale, hostname, and compile-time version.
    Status(status::Command),
    /// Settings catalog or a setting change.
    Settings(settings::Command),
}

/// Listeners, Wi-Fi lease, and in-memory token for this process.
pub(crate) struct ApiSession {
    /// Public listener, companion Wi-Fi lease, and mDNS record while on.
    public: Option<public::Running>,
    /// Always-on loopback listener.
    private: Option<Job<()>>,
    /// Bearer token for the public listener. Kept across suspend.
    token: Option<SecretString>,
    /// True between [`public::suspend`] and [`public::resume`].
    suspended: bool,
    /// Listeners queue commands here and then wake the main loop.
    tx: mpsc::UnboundedSender<Command>,
    /// Main loop drains queued commands from here.
    rx: mpsc::UnboundedReceiver<Command>,
    /// Port the loopback listener bound. Tests only.
    #[cfg(test)]
    private_addr: Option<SocketAddr>,
}

impl ApiSession {
    /// Empty session: no listeners, no token.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            public: None,
            private: None,
            token: None,
            suspended: false,
            tx,
            rx,
            #[cfg(test)]
            private_addr: None,
        }
    }

    /// True while the companion token is held, including during suspend.
    pub(crate) fn is_on(&self) -> bool {
        self.token.is_some()
    }

    #[cfg(test)]
    pub(crate) fn local_addr(&self) -> Option<SocketAddr> {
        self.public.as_ref().map(|running| running.local_addr)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(super) fn sender(&self) -> mpsc::UnboundedSender<Command> {
        self.tx.clone()
    }
}

/// Menu, hub, suspend, and resume events. Called from the binary event loop
/// before [`crate::device::DeviceLifecycle::handle_event`].
///
/// [`EventOutcome::Continue`] means the API did not consume the event, so the
/// caller must still run the device handler. Any other outcome means the API
/// consumed it and the device handler must be skipped.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(hub, bus, context, runtime),
        fields(event = ?event),
        ret(level = tracing::Level::TRACE)
    )
)]
pub async fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::Select(crate::view::EntryId::ToggleCompanion) => public::toggle(hub, context).await,
        Event::Api => {
            drain(hub, bus, context, runtime).await;
            EventOutcome::Handled
        }
        Event::ApiResume => public::resume(hub, context).await,
        Event::PrepareSuspend | Event::Suspend => {
            public::suspend(context).await;
            EventOutcome::Continue
        }
        _ => EventOutcome::Continue,
    }
}

/// Start the public listener on its default address with a fresh token.
///
/// The debug emulator turns the companion on at startup instead of through
/// the menu switch.
#[cfg(feature = "emulator")]
pub(crate) async fn turn_on_default(
    hub: &Hub,
    context: &mut AppContext,
) -> Result<(), public::StartError> {
    public::turn_on(hub, context, public::BIND, true).await
}

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to `/mnt/onboard/.adds`; other builds are
/// unrestricted.
#[cfg(feature = "kobo")]
pub(super) fn file_root(context: &AppContext) -> Option<std::path::PathBuf> {
    use crate::device::DevicePaths as _;

    context
        .device
        .install_dir()
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to `/mnt/onboard/.adds`; other builds are
/// unrestricted.
#[cfg(not(feature = "kobo"))]
pub(super) fn file_root(_context: &AppContext) -> Option<std::path::PathBuf> {
    None
}

/// Start the always-on loopback listener. Safe to call more than once.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub async fn start_private(hub: &Hub, context: &mut AppContext) {
    private::start(hub, context).await;
}

/// Cancel both listeners and drop the companion Wi-Fi lease.
///
/// Called from the binary event loop before mDNS shutdown, not from
/// [`crate::device::DeviceLifecycle::on_shutdown`].
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub async fn shutdown(context: &mut AppContext) {
    context.api.token = None;
    context.api.suspended = false;
    public::stop(&mut context.api).await;
    private::stop(&mut context.api).await;
}

/// Reply to queued listener commands on the main loop.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn drain(
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    while let Ok(command) = context.api.rx.try_recv() {
        match command {
            Command::Status(status::Command::Get { reply }) => {
                let _ = reply.send(Ok(status::current(context)));
            }
            Command::Settings(command) => {
                settings::handle(command, hub, bus, context, runtime).await;
            }
        }
    }
}
