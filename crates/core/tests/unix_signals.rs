//! Signal delivery tests.
//!
//! These live in an integration test so the unit-test binary keeps the
//! default signal dispositions: a Tokio signal handler stays installed for the
//! lifetime of the process, and the unit-test binary would otherwise stop
//! responding to Ctrl-C.

use std::time::Duration;

use cadmus_core::unix_signals::UnixSignals;
use cadmus_core::view::{EntryId, Event, Hub, HubReceiver};

/// Serializes the tests that signal the whole process.
static SIGNAL_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long a test waits for an event that must arrive.
const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a test waits before it calls an absent event a failure.
const NO_EVENT_TIMEOUT: Duration = Duration::from_millis(200);

fn test_hub() -> (Hub, HubReceiver) {
    tokio::sync::mpsc::unbounded_channel()
}

/// Sends a signal to this process.
///
/// The caller installs a listener for the signal first, so the default action
/// is suppressed and the signal cannot end the test process.
fn send(sig: i32) {
    // SAFETY: the signal goes to this process, whose handler is installed
    // before the call, so the default action is suppressed.
    unsafe { libc::kill(libc::getpid(), sig) };
}

async fn next_event(rx: &mut HubReceiver) -> Event {
    tokio::time::timeout(EVENT_TIMEOUT, rx.recv())
        .await
        .expect("the signal did not reach the hub")
        .expect("the hub is closed")
        .event
}

#[tokio::test]
async fn sigusr1_reaches_the_hub_as_restart() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let _listener = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
        .expect("failed to listen for SIGUSR1");

    let signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGUSR1);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Restart)));
}

#[tokio::test]
async fn sigterm_reaches_the_hub_as_quit() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let _listener = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to listen for SIGTERM");

    let signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGTERM);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));
}

#[tokio::test]
async fn a_later_shutdown_signal_is_ignored() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to listen for SIGTERM");
    let _usr1 = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())
        .expect("failed to listen for SIGUSR1");

    let signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGTERM);
    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));

    send(libc::SIGUSR1);
    let later = tokio::time::timeout(NO_EVENT_TIMEOUT, rx.recv()).await;
    assert!(later.is_err(), "the later signal was not ignored");
}

#[tokio::test]
async fn a_refused_exit_allows_another_request() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to listen for SIGTERM");

    let signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGTERM);
    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));

    signals.reset();

    send(libc::SIGTERM);
    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));
}
