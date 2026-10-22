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
//! so `attach()` hands it over later. A shutdown signal that arrives in
//! between is held and posted as soon as the hub arrives.
//!
//! # Rules
//!
//! The first shutdown signal decides. Later shutdown signals are logged and
//! ignored, so a restart request can never turn a pending quit into a
//! restart. A signal the device refuses (the Full inhibit blocks Quit and
//! Restart) is re-armed by [`UnixSignals::reset`], so the next signal can ask
//! again.

#[cfg(unix)]
mod imp {
    use std::fmt;
    use std::future::pending;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::signal::unix::{Signal as SignalStream, SignalKind, signal as listen};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::runtime::Job;
    use crate::view::{EntryId, Event, Hub};

    /// How long the listener gets to notice the cancellation before it is aborted.
    const STOP_DEADLINE: Duration = Duration::from_millis(500);

    /// The process signals Cadmus reacts to.
    #[derive(Clone, Copy, PartialEq, Eq)]
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
    }

    impl fmt::Display for Signal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.name())
        }
    }

    impl fmt::Debug for Signal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.name())
        }
    }

    /// Handlers for the process signals Cadmus reacts to.
    ///
    /// The setup is two steps because the handler and the hub are not ready at
    /// the same time. Call [`UnixSignals::install`] as early as the process
    /// can, so a signal during startup is captured instead of killing the
    /// process with the default disposition, then call
    /// [`UnixSignals::attach`] once the hub exists. A shutdown signal that
    /// arrives before the hub is delivered as soon as the hub arrives.
    ///
    /// The listener stops when the value is dropped. [`UnixSignals::stop`]
    /// stops it and waits for it to finish.
    pub struct UnixSignals {
        job: Job<()>,
        hub_tx: mpsc::Sender<Hub>,
        live: Vec<Signal>,
        requested: Arc<AtomicBool>,
    }

    impl UnixSignals {
        /// Installs the OS handlers and starts the listener.
        ///
        /// The handlers are installed synchronously, so a signal that arrives
        /// before the hub exists is captured instead of killing the process.
        /// A signal that cannot be listened to is skipped with a warning; the
        /// other signals still work.
        ///
        /// The hub is handed over later with [`UnixSignals::attach`].
        pub fn install() -> Self {
            let term = try_listen(Signal::Term);
            let interrupt = try_listen(Signal::Interrupt);
            let usr1 = try_listen(Signal::UserDefined1);
            let hangup = try_listen(Signal::Hangup);

            let live: Vec<Signal> = [
                (Signal::Term, term.is_some()),
                (Signal::Interrupt, interrupt.is_some()),
                (Signal::UserDefined1, usr1.is_some()),
                (Signal::Hangup, hangup.is_some()),
            ]
            .into_iter()
            .filter_map(|(signal, is_live)| is_live.then_some(signal))
            .collect();

            let (hub_tx, hub_rx) = mpsc::channel(1);
            let requested = Arc::new(AtomicBool::new(false));
            let job_requested = Arc::clone(&requested);
            let job = Job::spawn(move |cancel| {
                run(term, interrupt, usr1, hangup, hub_rx, job_requested, cancel)
            });

            Self {
                job,
                hub_tx,
                live,
                requested,
            }
        }

        /// Hands the hub to the listener.
        ///
        /// The hub only exists once the device input pipeline has started, so
        /// this is a separate step from [`UnixSignals::install`]. A shutdown
        /// signal received before this call is delivered as soon as the hub
        /// arrives.
        pub fn attach(&self, hub: Hub) {
            tracing::info!(signals = ?self.live, "attached the hub to the Unix signal listener");
            if self.hub_tx.try_send(hub).is_err() {
                tracing::warn!("the signal listener is gone; the hub was not attached");
            }
        }

        /// Stops the listener and waits for it to finish.
        pub async fn stop(self) {
            self.job.cancel();
            let _ = self.job.join(STOP_DEADLINE).await;
        }

        /// Allows another exit request after the device refused one.
        ///
        /// A shutdown signal that the Full inhibit refuses leaves the request
        /// without effect, so the next signal has to be able to ask again.
        pub fn reset(&self) {
            self.requested.store(false, Ordering::SeqCst);
        }
    }

    async fn run(
        mut term: Option<SignalStream>,
        mut interrupt: Option<SignalStream>,
        mut usr1: Option<SignalStream>,
        mut hangup: Option<SignalStream>,
        mut hub_rx: mpsc::Receiver<Hub>,
        requested: Arc<AtomicBool>,
        cancel: CancellationToken,
    ) {
        let mut hub: Option<Hub> = None;
        let mut pending: Option<Event> = None;

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                Some(received) = hub_rx.recv() => {
                    hub = Some(received);
                    deliver_pending(&hub, &mut pending, &requested);
                }
                Some(()) = recv_optional(&mut term) => {
                    on_signal(Signal::Term, &hub, &mut pending, &requested);
                }
                Some(()) = recv_optional(&mut interrupt) => {
                    on_signal(Signal::Interrupt, &hub, &mut pending, &requested);
                }
                Some(()) = recv_optional(&mut usr1) => {
                    on_signal(Signal::UserDefined1, &hub, &mut pending, &requested);
                }
                Some(()) = recv_optional(&mut hangup) => {
                    on_signal(Signal::Hangup, &hub, &mut pending, &requested);
                }
            }
        }
    }

    /// Records a signal and asks for the exit it implies.
    ///
    /// The first shutdown signal wins. A later one is ignored so a restart
    /// request cannot turn a pending quit into a restart. Before the hub is
    /// attached the request is held until the hub arrives.
    fn on_signal(
        signal: Signal,
        hub: &Option<Hub>,
        pending: &mut Option<Event>,
        requested: &AtomicBool,
    ) {
        let Some(event) = signal.event() else {
            tracing::info!(signal = %signal, "ignoring Unix signal");
            return;
        };

        if requested.load(Ordering::SeqCst) || pending.is_some() {
            tracing::info!(signal = %signal, "ignoring signal after an exit request");
            return;
        }

        match hub {
            Some(hub) => {
                tracing::info!(signal = %signal, "Unix signal requests an exit");
                requested.store(true, Ordering::SeqCst);
                post(hub, event);
            }
            None => {
                tracing::info!(
                    signal = %signal,
                    "holding the exit request until the hub is ready"
                );
                *pending = Some(event);
            }
        }
    }

    /// Posts a held exit request once the hub is available.
    fn deliver_pending(hub: &Option<Hub>, pending: &mut Option<Event>, requested: &AtomicBool) {
        let Some(hub) = hub.as_ref() else {
            return;
        };
        let Some(event) = pending.take() else {
            return;
        };
        requested.store(true, Ordering::SeqCst);
        post(hub, event);
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
                tracing::warn!(signal = %signal, error = %error, "failed to listen for Unix signal");
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn sigterm_selects_quit() {
            assert!(matches!(
                Signal::Term.event(),
                Some(Event::Select(EntryId::Quit))
            ));
        }

        #[test]
        fn sigint_selects_quit() {
            assert!(matches!(
                Signal::Interrupt.event(),
                Some(Event::Select(EntryId::Quit))
            ));
        }

        #[test]
        fn sigusr1_selects_restart() {
            assert!(matches!(
                Signal::UserDefined1.event(),
                Some(Event::Select(EntryId::Restart))
            ));
        }

        #[test]
        fn sighup_is_ignored() {
            assert!(Signal::Hangup.event().is_none());
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use crate::view::Hub;

    /// Signal handling is not available on this platform.
    pub struct UnixSignals;

    impl UnixSignals {
        /// Does nothing on platforms without Unix signals.
        pub fn install() -> Self {
            tracing::info!("Unix signal handling is not supported on this platform");
            Self
        }

        /// Does nothing on platforms without Unix signals.
        pub fn attach(&self, _hub: Hub) {}

        /// Does nothing on platforms without Unix signals.
        pub fn reset(&self) {}

        /// Does nothing on platforms without Unix signals.
        pub async fn stop(self) {}
    }
}

pub use imp::UnixSignals;
