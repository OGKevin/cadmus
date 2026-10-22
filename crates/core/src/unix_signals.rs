//! Unix process signals mapped onto the same hub events as the top-menu Quit
//! and Restart commands.
//!
//! SIGTERM and SIGINT post [`Event::Select`](crate::view::Event::Select) with
//! [`EntryId::Quit`](crate::view::EntryId::Quit). SIGUSR1 posts Restart.
//! SIGHUP is ignored so an SSH hangup does not stop Cadmus.
//!
//! # Two-step setup
//!
//! [`UnixSignals::install`] and [`UnixSignals::attach`] are called at
//! different points on purpose.
//!
//! `install()` runs as early as the process can, before the device is opened.
//! Registering an OS handler is what replaces the default disposition of a
//! signal, so until it happens a `SIGTERM` still kills the process outright
//! (exit 143) and a `SIGHUP` still ends it. Installing the handler late
//! therefore leaves the whole startup window unprotected, and a supervisor
//! that stops Cadmus during startup would produce a crash instead of a clean
//! exit.
//!
//! The hub does not exist that early. It comes from the device input
//! pipeline, which only starts once the context and the databases are built,
//! so `attach()` hands it over later. Tokio buffers signals received between
//! those calls; the listener posts them as soon as it runs.
//!
//! # Rules
//!
//! The first shutdown signal decides. A later SIGUSR1 is logged and ignored
//! so a restart request can never turn a pending quit into a restart. A later
//! SIGINT or SIGTERM escalates: the process exits with `128 + signo`. A signal
//! the device refuses (the Full inhibit blocks Quit and Restart) is re-armed
//! by [`UnixSignals::reset`], so the next signal can ask again.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::signal::unix::{Signal as SignalStream, SignalKind, signal as listen};
use tokio_util::sync::CancellationToken;

use crate::runtime::Job;
use crate::view::{EntryId, Event, Hub};

/// How long the listener gets to notice the cancellation before it is aborted.
const STOP_DEADLINE: Duration = Duration::from_millis(500);

/// The process signals Cadmus reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Term,
    Interrupt,
    UserDefined1,
    Hangup,
}

impl Signal {
    /// The name of the signal.
    fn name(self) -> &'static str {
        match self {
            Signal::Term => "SIGTERM",
            Signal::Interrupt => "SIGINT",
            Signal::UserDefined1 => "SIGUSR1",
            Signal::Hangup => "SIGHUP",
        }
    }

    /// The tokio signal kind that listens for this signal.
    fn kind(self) -> SignalKind {
        match self {
            Signal::Term => SignalKind::terminate(),
            Signal::Interrupt => SignalKind::interrupt(),
            Signal::UserDefined1 => SignalKind::user_defined1(),
            Signal::Hangup => SignalKind::hangup(),
        }
    }

    /// The event this signal asks for.
    ///
    /// SIGHUP asks for nothing: a hangup is not a request to stop Cadmus.
    fn event(self) -> Option<Event> {
        match self {
            Signal::Term | Signal::Interrupt => Some(Event::Select(EntryId::Quit)),
            Signal::UserDefined1 => Some(Event::Select(EntryId::Restart)),
            Signal::Hangup => None,
        }
    }

    fn signo(self) -> i32 {
        match self {
            Signal::Term => libc::SIGTERM,
            Signal::Interrupt => libc::SIGINT,
            Signal::UserDefined1 => libc::SIGUSR1,
            Signal::Hangup => libc::SIGHUP,
        }
    }
}

/// What the listener should do with a received signal.
#[derive(Debug)]
enum Decision {
    Ignore,
    Post(Event),
    Escalate,
}

fn decide(signal: Signal, already_requested: bool) -> Decision {
    if already_requested {
        return match signal {
            Signal::Term | Signal::Interrupt => Decision::Escalate,
            Signal::UserDefined1 | Signal::Hangup => Decision::Ignore,
        };
    }
    match signal.event() {
        Some(event) => Decision::Post(event),
        None => Decision::Ignore,
    }
}

/// Handlers for the process signals Cadmus reacts to.
///
/// The listener stops when the value is dropped. [`UnixSignals::stop`]
/// stops it and waits for it to finish.
pub struct UnixSignals {
    term: Option<SignalStream>,
    interrupt: Option<SignalStream>,
    usr1: Option<SignalStream>,
    hangup: Option<SignalStream>,
    job: Option<Job<()>>,
    requested: Arc<AtomicBool>,
}

impl UnixSignals {
    /// Registers the OS handlers. The hub is handed over later with [`UnixSignals::attach`].
    ///
    /// A signal that cannot be listened to is skipped with a warning; the
    /// other signals still work.
    pub fn install() -> Self {
        Self {
            term: try_listen(Signal::Term),
            interrupt: try_listen(Signal::Interrupt),
            usr1: try_listen(Signal::UserDefined1),
            hangup: try_listen(Signal::Hangup),
            job: None,
            requested: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts the listener with `hub`. Signals Tokio already buffered are posted then.
    pub fn attach(&mut self, hub: Hub) {
        if self.job.is_some() {
            tracing::warn!("Unix signal listener already attached");
            return;
        }
        let term = self.term.take();
        let interrupt = self.interrupt.take();
        let usr1 = self.usr1.take();
        let hangup = self.hangup.take();
        let requested = Arc::clone(&self.requested);
        self.job = Some(Job::spawn(move |cancel| {
            run(term, interrupt, usr1, hangup, hub, requested, cancel)
        }));
        tracing::info!("attached the hub to the Unix signal listener");
    }

    /// Stops the listener and waits for it to finish.
    pub async fn stop(mut self) {
        if let Some(job) = self.job.take() {
            job.cancel();
            let _ = job.join(STOP_DEADLINE).await;
        }
        drop(self);
        restore_default_termination();
    }

    /// Allows another exit request after the device refused one.
    ///
    /// A shutdown signal that the Full inhibit or a USB share refuses leaves
    /// the request without effect, so the next signal has to be able to ask
    /// again.
    pub fn reset(&self) {
        self.requested.store(false, Ordering::SeqCst);
    }
}

async fn run(
    mut term: Option<SignalStream>,
    mut interrupt: Option<SignalStream>,
    mut usr1: Option<SignalStream>,
    mut hangup: Option<SignalStream>,
    hub: Hub,
    requested: Arc<AtomicBool>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            Some(()) = recv_optional(&mut term) => {
                on_signal(Signal::Term, &hub, &requested);
            }
            Some(()) = recv_optional(&mut interrupt) => {
                on_signal(Signal::Interrupt, &hub, &requested);
            }
            Some(()) = recv_optional(&mut usr1) => {
                on_signal(Signal::UserDefined1, &hub, &requested);
            }
            Some(()) = recv_optional(&mut hangup) => {
                on_signal(Signal::Hangup, &hub, &requested);
            }
        }
    }
}

fn on_signal(signal: Signal, hub: &Hub, requested: &AtomicBool) {
    match decide(signal, requested.load(Ordering::SeqCst)) {
        Decision::Ignore => {
            tracing::info!(signal = signal.name(), "ignoring Unix signal");
        }
        Decision::Post(event) => {
            tracing::info!(signal = signal.name(), "Unix signal requests an exit");
            requested.store(true, Ordering::SeqCst);
            post(hub, event);
        }
        Decision::Escalate => {
            tracing::warn!(signal = signal.name(), "escalating repeated Unix signal");
            std::process::exit(128 + signal.signo());
        }
    }
}

fn post(hub: &Hub, event: Event) {
    if hub.send(event.into()).is_err() {
        tracing::warn!("the hub is gone; the signal event was dropped");
    }
}

fn try_listen(signal: Signal) -> Option<SignalStream> {
    match listen(signal.kind()) {
        Ok(stream) => Some(stream),
        Err(error) => {
            tracing::warn!(
                signal = signal.name(),
                error = %error,
                "failed to listen for Unix signal"
            );
            None
        }
    }
}

async fn recv_optional(stream: &mut Option<SignalStream>) -> Option<()> {
    match stream.as_mut() {
        Some(signal) => signal.recv().await,
        None => pending().await,
    }
}

/// Restores SIGTERM and SIGINT to SIG_DFL so a hung teardown can still be killed.
///
/// Tokio leaves its handlers installed after the listener job stops. Call this
/// only after that job has been joined (or was never started).
///
/// # Safety
///
/// The listener has been joined or never spawned, so nothing in this process
/// is awaiting these tokio::signal streams. `SIG_DFL` is the POSIX default
/// disposition and makes a later SIGTERM/SIGINT terminate the process.
fn restore_default_termination() {
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    enum Want {
        Ignore,
        Quit,
        Restart,
        Escalate,
    }

    fn classify(decision: Decision) -> Want {
        match decision {
            Decision::Ignore => Want::Ignore,
            Decision::Post(Event::Select(EntryId::Quit)) => Want::Quit,
            Decision::Post(Event::Select(EntryId::Restart)) => Want::Restart,
            Decision::Escalate => Want::Escalate,
            Decision::Post(event) => panic!("unexpected event {event:?}"),
        }
    }

    #[test]
    fn decide_table() {
        let cases = [
            (Signal::Hangup, false, Want::Ignore),
            (Signal::Hangup, true, Want::Ignore),
            (Signal::Term, false, Want::Quit),
            (Signal::Interrupt, false, Want::Quit),
            (Signal::UserDefined1, false, Want::Restart),
            (Signal::Term, true, Want::Escalate),
            (Signal::Interrupt, true, Want::Escalate),
            (Signal::UserDefined1, true, Want::Ignore),
        ];
        for (signal, already, expected) in cases {
            assert_eq!(
                classify(decide(signal, already)),
                expected,
                "{}",
                signal.name()
            );
        }
    }
}
