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

/// Sends a signal to this process after [`UnixSignals::install`] has registered handlers.
///
/// # Safety
///
/// `install()` has replaced the default disposition for this signal.
fn send(sig: i32) {
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
    let mut signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGUSR1);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Restart)));
}

#[tokio::test]
async fn sigterm_reaches_the_hub_as_quit() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGTERM);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));
}

#[tokio::test]
async fn sigint_reaches_the_hub_as_quit() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGINT);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));
}

#[tokio::test]
async fn a_later_usr1_is_ignored_after_term() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
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
    let mut signals = UnixSignals::install();
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

#[tokio::test]
async fn a_term_before_attach_still_reaches_the_hub() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
    send(libc::SIGTERM);
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));
}

#[tokio::test]
async fn hangup_is_ignored_and_term_still_quits_once() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
    let (tx, mut rx) = test_hub();
    signals.attach(tx);

    send(libc::SIGHUP);
    send(libc::SIGTERM);

    let event = next_event(&mut rx).await;
    assert!(matches!(event, Event::Select(EntryId::Quit)));

    let extra = tokio::time::timeout(NO_EVENT_TIMEOUT, rx.recv()).await;
    assert!(extra.is_err(), "an extra event arrived after Quit");
}

#[tokio::test]
async fn stop_completes() {
    let _guard = SIGNAL_TEST_LOCK.lock().await;
    let mut signals = UnixSignals::install();
    let (tx, _rx) = test_hub();
    signals.attach(tx);
    signals.stop().await;
}
