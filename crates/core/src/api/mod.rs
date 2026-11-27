//! Public and private Cadmus APIs.
//!
//! Shared command dispatch, settings catalog, file ops, and status live here.
//! The public listener is on-demand (see [`CompanionView`](crate::view::companion::CompanionView)). The private
//! loopback listener starts with the process. Handlers do not touch
//! [`AppContext`]. They queue a command and wake the main loop, which replies
//! on a oneshot.

mod files;
mod internal_rpc;
cfg_select! {
    feature = "bench" => { pub mod logs; }
    _ => { mod logs; }
}
mod private;
mod public;
mod public_rpc;
mod serve;
mod settings;
mod shared_rpc;
mod status;

#[cfg(test)]
mod tests;

use crate::device::{AppContext, DeviceRuntime, EventOutcome};
use crate::framebuffer::UpdateMode;
use crate::runtime::Job;
use crate::view::{Bus, Event, Hub, RenderData, RenderQueue, View, ViewId};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(test)]
use std::net::SocketAddr;

#[cfg(test)]
use settings::toggle_value;

use cadmus_api_proto::cadmus::api::shared::v1 as proto;

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const JOIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const LEASE_NAME: &str = "public-api";

/// Bad-token requests the public listener tolerates before it shuts down.
///
/// Three consecutive bad credentials are enough to cover a mistyped
/// six-digit token, and few enough that guessing the whole space is
/// hopeless. A matching credential clears the count, so re-entering the
/// code starts over instead of compounding earlier mistakes. Debug builds
/// skip the bearer check entirely, so this only guards devices an attacker
/// can reach.
const MAX_AUTH_FAILURES: u32 = 3;

/// Work a listener asks the main loop to do.
enum Command {
    /// Locale, hostname, and compile-time version.
    Status(status::Command),
    /// Settings catalog or a setting change.
    Settings(settings::Command),
    /// A request presented a bearer credential the listener rejected.
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
    /// Rejected credentials since the public listener last minted a token.
    ///
    /// Listeners increment this synchronously before answering, so a locked
    /// out listener rejects requests before the main loop drains the notice.
    auth_failures: Arc<AtomicU32>,
    /// Job that is taking the Wi-Fi lease and binding the port, if any.
    pending_start: Option<public::PendingStart>,
    /// True while a [`CompanionView`](crate::view::companion::CompanionView) is owed to a Companion start the user
    /// requested from the main menu.
    ///
    /// The listener binds asynchronously, so the view can only show the real
    /// URL (including a non-80 emulator port) after `Command::ListenerBound`
    /// arrives. The menu entry sets this flag. The matching bind result
    /// consumes it and pushes the view if the listener came up. Stop, suspend
    /// and a second menu choice clear it with the pending start. The emulator's
    /// automatic startup session and suspend/resume never set it, so they
    /// never open the view.
    view_pending: bool,
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
            auth_failures: Arc::new(AtomicU32::new(0)),
            pending_start: None,
            view_pending: false,
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

    /// The bearer token, shown once in the on-device [`CompanionView`](crate::view::companion::CompanionView). Never log it.
    pub(crate) fn token(&self) -> Option<String> {
        self.token
            .as_ref()
            .map(|token| token.expose_secret().to_string())
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
        self.view_pending = false;
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
        skip(hub, bus, rq, context, runtime),
        fields(event = ?event),
        ret(level = tracing::Level::TRACE)
    )
)]
pub async fn handle_event(
    event: &Event,
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::Select(crate::view::EntryId::OpenCompanion) => {
            let outcome = public::open(hub, context).await;
            if context.api.is_on() {
                context.api.view_pending = true;
            }
            outcome
        }
        Event::Api => {
            drain(hub, bus, rq, context, runtime).await;
            EventOutcome::Handled
        }
        Event::ApiResume => public::resume(hub, context).await,
        Event::PrepareSuspend | Event::Suspend => {
            public::suspend(context).await;
            EventOutcome::Continue
        }
        Event::PrepareShare | Event::Share => {
            if context.api.is_on() {
                public::turn_off(context).await;
                hub.send(Event::Close(ViewId::Companion).into()).ok();
            }
            EventOutcome::Continue
        }
        Event::Close(ViewId::Companion) => {
            public::turn_off(context).await;
            EventOutcome::Continue
        }
        _ => EventOutcome::Continue,
    }
}

/// Shows the full-screen [`CompanionView`](crate::view::companion::CompanionView) after the menu entry turned the
/// listener on.
///
/// The view is pushed at the root view level, like `AboutDialog`, so
/// `Event::Close` reaches it: `locate_by_id` only searches direct children.
/// The debug emulator's automatic startup session and suspend/resume must not
/// open the view; only this menu path does.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn push_companion_view(
    rq: &mut RenderQueue,
    context: &mut AppContext,
    view: &mut Box<dyn View>,
) {
    let Some(pin) = context.api.token() else {
        return;
    };
    if crate::view::common::locate_by_id(view.as_ref(), ViewId::Companion).is_some() {
        return;
    }
    let Some(url) = public::companion_url(context) else {
        tracing::warn!("companion view url could not be resolved");
        return;
    };
    let ip_url = public::companion_ip_url(context);
    let awake = public::companion_keeps_awake(context);
    let Some(dialog) =
        crate::view::companion::CompanionView::new(context, &url, ip_url.as_deref(), &pin, awake)
    else {
        tracing::warn!("companion view could not be built");
        return;
    };
    rq.add(RenderData::new(
        dialog.id(),
        *dialog.rect(),
        UpdateMode::Full,
    ));
    view.children_mut().push(Box::new(dialog) as Box<dyn View>);
}

/// Start the public listener on loopback with a fresh token.
///
/// The debug emulator turns the companion on at startup instead of through
/// the menu entry. It binds loopback only, so a dev machine never exposes
/// file and settings RPCs to the LAN, and the loopback address skips mDNS.
#[cfg(feature = "emulator")]
pub(crate) async fn turn_on_default(hub: &Hub, context: &mut AppContext) {
    if let Err(error) = public::turn_on(hub, context, DEBUG_BIND, true).await {
        tracing::warn!(error = %error, "debug emulator companion did not start");
    }
}

#[cfg(feature = "emulator")]
const DEBUG_BIND: std::net::SocketAddr =
    std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to `/mnt/onboard`, where books, default
/// libraries, and user sleep images live. Tests keep the temporary install
/// root so they never touch a real card. Other builds stay inside the device
/// install directory so a LAN client can never read the host filesystem.
#[cfg(feature = "kobo")]
fn file_root(_context: &AppContext) -> Option<std::path::PathBuf> {
    cfg_select! {
        test => {
            use crate::device::DevicePaths as _;

            _context
                .device
                .install_dir()
                .parent()
                .map(std::path::Path::to_path_buf)
        }
        _ => Some(std::path::PathBuf::from(crate::settings::INTERNAL_CARD_ROOT)),
    }
}

/// Uppermost directory the file RPCs may read on this device.
///
/// Kobo builds confine browsing to `/mnt/onboard`; other builds stay inside
/// the device install directory so a LAN client can never read the host
/// filesystem.
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
    context.api.auth_failures.store(0, Ordering::Relaxed);
}

/// Reply to queued listener commands on the main loop.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
async fn drain(
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
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
                let show_view = generation == context.api.start_generation
                    && std::mem::take(&mut context.api.view_pending);
                public::finish_start(hub, context, generation, result).await;
                if show_view && context.api.is_on() {
                    push_companion_view(rq, context, runtime.view).await;
                }
            }
            Command::AuthFailed => {
                let failures = context.api.auth_failures.load(Ordering::Relaxed);
                if failures >= MAX_AUTH_FAILURES {
                    tracing::warn!(failures, "public api disabled after repeated bad tokens");
                    public::turn_off(context).await;
                    hub.send((Event::Close(ViewId::Companion)).into()).ok();
                }
            }
        }
    }
}
