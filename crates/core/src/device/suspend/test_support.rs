//! Shared helpers for suspend orchestrator and Kobo lifecycle unit tests.

use crate::device::EventOutcome;
use crate::device::rtc::AlarmManager;
use crate::device::suspend::handle_event;
use crate::device::test_harness::DeviceRuntimeHarness;
use crate::view::intermission::Intermission;
use crate::view::{Event, View};

pub(crate) fn lock_alarms(
    harness: &mut DeviceRuntimeHarness,
) -> std::sync::MutexGuard<'_, AlarmManager<crate::device::rtc::TestRtc>> {
    harness
        .context
        .alarm_manager
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
}

pub(crate) fn intermission_count(view: &dyn View) -> usize {
    view.children()
        .iter()
        .filter(|child| child.as_ref().is::<Intermission>())
        .count()
}

pub(crate) fn install_armed_soft_suspend(
    harness: &mut DeviceRuntimeHarness,
) -> (
    tempfile::TempDir,
    crate::device::linux::soft_suspend::paths::SoftSuspendPaths,
) {
    use crate::device::inhibitor::Inhibitor;
    use crate::device::linux::soft_suspend::paths::SoftSuspendPaths;
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use crate::device::soft_suspend::mode::AutosleepMode;
    use std::sync::Arc;

    let (dir, paths) = SoftSuspendPaths::test_fixture();
    let inhibitor = Inhibitor::with_paths(paths.clone(), None);
    inhibitor.set_mode(AutosleepMode::Freeze);
    harness
        .context
        .wifi_session
        .set_inhibitor(Arc::clone(&inhibitor));
    harness.context.inhibitor = inhibitor;
    (dir, paths)
}

pub(crate) fn pump_deep_idle_wake(harness: &mut DeviceRuntimeHarness) {
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_some(),
        "deep-idle wait must be armed before pump"
    );
    let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
        handle_event(&Event::PollDeepIdleWait, hub, bus, rq, context, runtime)
    });
    assert_eq!(outcome, EventOutcome::Handled);
    assert!(
        harness
            .context
            .suspend
            .as_ref()
            .and_then(|c| c.deep_idle_wait())
            .is_none()
    );
}
