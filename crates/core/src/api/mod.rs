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
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const LEASE_NAME: &str = "public-api";

/// Bad-token requests the public listener tolerates before it shuts down.
///
/// Three strikes are enough to cover a mistyped four-digit token, and few
/// enough that guessing the whole space is hopeless.
const MAX_AUTH_FAILURES: u32 = 3;

/// Work a listener asks the main loop to do.
enum Command {
    /// Locale, hostname, and compile-time version.
    Status(status::Command),
    /// Settings catalog or a setting change.
    Settings(settings::Command),
    /// A request presented a token the listener rejected.
    AuthFailed,
    /// The public listener finished taking Wi-Fi and binding.
    ListenerBound {
        generation: u64,
        result: Result<public::BoundListener, public::StartError>,
    },
}

/// Listeners, Wi-Fi lease, and in-memory token for this process.
pub(crate) struct ApiSession {
    /// Public listener, companion Wi-Fi lease, and mDNS record while on.
    public: Option<public::Running>,
    /// Always-on loopback listener.
    private: Option<Job<()>>,
    /// Bearer token for the public listener. Kept across suspend.
    token: Option<SecretString>,
    /// Rejected tokens since the public listener last started.
    auth_failures: u32,
    /// Job that is taking the Wi-Fi lease and binding the port, if any.
    pending_start: Option<public::PendingStart>,
    /// Bumped per start attempt so a superseded result is ignored.
    start_generation: u64,
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
            auth_failures: 0,
            pending_start: None,
            start_generation: 0,
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

    /// Bump the start generation and return the new value.
    fn begin_start(&mut self) -> u64 {
        self.start_generation = self.start_generation.wrapping_add(1);
        self.start_generation
    }

    /// Abandon a pending start and return it so the caller can join it.
    fn take_pending_start(&mut self) -> Option<public::PendingStart> {
        self.start_generation = self.start_generation.wrapping_add(1);
        self.pending_start.take()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn sender(&self) -> mpsc::UnboundedSender<Command> {
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

/// Start the public listener on loopback with a fresh token.
///
/// The debug emulator turns the companion on at startup instead of through
/// the menu switch. It binds loopback only, so a dev machine never exposes
/// file and settings RPCs to the LAN, and the loopback address skips mDNS.
#[cfg(feature = "emulator")]
pub(crate) async fn turn_on_default(
    hub: &Hub,
    context: &mut AppContext,
) -> Result<(), public::StartError> {
    public::turn_on(hub, context, DEBUG_BIND, true).await
}

#[cfg(feature = "emulator")]
const DEBUG_BIND: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to the parent of `/mnt/onboard/.adds`; other
/// builds stay inside the device install directory so a LAN client can never
/// read the host filesystem.
#[cfg(feature = "kobo")]
fn file_root(context: &AppContext) -> Option<std::path::PathBuf> {
    use crate::device::DevicePaths as _;

    context
        .device
        .install_dir()
        .parent()
        .map(std::path::Path::to_path_buf)
}

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to the parent of `/mnt/onboard/.adds`; other
/// builds stay inside the device install directory so a LAN client can never
/// read the host filesystem.
#[cfg(not(feature = "kobo"))]
fn file_root(context: &AppContext) -> Option<std::path::PathBuf> {
    use crate::device::DevicePaths as _;

    Some(context.device.install_dir())
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
            Command::Settings(command) if command.reply_is_closed() => {}
            Command::Settings(command) => {
                settings::handle(command, hub, bus, context, runtime).await;
            }
            Command::ListenerBound { generation, result } => {
                public::finish_start(hub, context, generation, result).await;
            }
            Command::AuthFailed => {
                context.api.auth_failures = context.api.auth_failures.saturating_add(1);
                if context.api.auth_failures >= MAX_AUTH_FAILURES {
                    tracing::warn!(
                        failures = context.api.auth_failures,
                        "public api disabled after repeated bad tokens"
                    );
                    public::turn_off(context).await;
                }
            }
        }
    }
}
