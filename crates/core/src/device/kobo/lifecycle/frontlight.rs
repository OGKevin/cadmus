//! Frontlight toggle and level adjustment event handling.

use crate::chrono::Local;
use crate::device::DeviceHardware as _;
use crate::device::{AppContext, DeviceRuntime, EventOutcome};
use crate::frontlight::Frontlight as _;
use crate::frontlight::LightLevels;
use crate::task::{TaskError, TaskId};
use crate::view::{Event, Hub, RenderQueue};

/// Dispatches frontlight-related lifecycle events.
pub(super) fn handle_event(
    event: &Event,
    _hub: &Hub,
    _bus: &mut crate::view::Bus,
    _rq: &mut RenderQueue,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) -> EventOutcome {
    match event {
        Event::ToggleFrontlight => {
            handle_toggle_frontlight(context);
            EventOutcome::Continue
        }
        Event::SetFrontlightLevels(levels) => {
            handle_set_frontlight_levels(levels, context, runtime);
            EventOutcome::Handled
        }
        Event::UpdateAutoFrontlight => {
            handle_update_auto_frontlight(context);
            EventOutcome::Handled
        }
        _ => EventOutcome::Unhandled,
    }
}

fn handle_toggle_frontlight(context: &mut AppContext) {
    context.set_frontlight(!context.settings.frontlight);
}

fn handle_set_frontlight_levels(
    levels: &LightLevels,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    if let Some(background_tasks) = runtime.background_tasks.as_mut()
        && let Err(error) = background_tasks.stop(&TaskId::AutoFrontlight)
        && !matches!(error, TaskError::NotRunning(TaskId::AutoFrontlight))
    {
        tracing::warn!(error = %error, "failed to stop auto_frontlight task after manual adjustment");
    }

    if let Err(error) = context
        .device
        .frontlight_mut()
        .set_intensity(levels.intensity)
    {
        tracing::error!(error = %error, "failed to set frontlight intensity");
    }
    if let Err(error) = context.device.frontlight_mut().set_warmth(levels.warmth) {
        tracing::error!(error = %error, "failed to set frontlight warmth");
    }
    context.settings.frontlight_levels = *levels;
}

fn handle_update_auto_frontlight(context: &mut AppContext) {
    if !(context.settings.auto_frontlight && context.settings.frontlight) {
        return;
    }

    let Some(coords) = crate::settings::resolve_coordinates(&context.settings) else {
        tracing::debug!("no coordinates available for auto-frontlight");
        return;
    };

    let night_brightness = context
        .settings
        .auto_frontlight_night_brightness
        .unwrap_or_default();
    let current_intensity = context.device.frontlight().levels().intensity;
    let levels = crate::frontlight::auto::compute_auto_frontlight_levels(
        Local::now(),
        coords,
        night_brightness,
        current_intensity,
    );
    if let Err(error) = context
        .device
        .frontlight_mut()
        .set_intensity(levels.intensity)
    {
        tracing::error!(error = %error, "failed to set auto frontlight intensity");
    }
    if let Err(error) = context.device.frontlight_mut().set_warmth(levels.warmth) {
        tracing::error!(error = %error, "failed to set auto frontlight warmth");
    }
    context.settings.frontlight_levels = levels;
}

#[cfg(all(test, feature = "kobo"))]
mod tests {
    use super::*;
    use crate::device::DeviceRuntime;
    use crate::device::kobo::lifecycle::test_helpers::LifecycleHarness;
    use crate::frontlight::LightLevels;
    use crate::task::{BackgroundTask, ShutdownSignal, TaskId, TaskManager};
    use crate::view::Event;
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    struct WaitingTask;

    impl BackgroundTask for WaitingTask {
        fn id(&self) -> TaskId {
            TaskId::AutoFrontlight
        }

        fn run(&mut self, _hub: &Sender<Event>, shutdown: &ShutdownSignal) {
            shutdown.wait(Duration::from_secs(60));
        }
    }

    #[test]
    fn handle_event_toggle_frontlight_updates_settings() {
        let mut harness = LifecycleHarness::new();
        harness.context.settings.frontlight = false;
        let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
            handle_event(&Event::ToggleFrontlight, hub, bus, rq, context, runtime)
        });
        assert_eq!(outcome, EventOutcome::Continue);
        assert!(harness.context.settings.frontlight);
    }

    #[test]
    fn handle_event_set_frontlight_levels_stops_auto_task() {
        let mut harness = LifecycleHarness::new();
        let mut background_tasks = TaskManager::new();
        background_tasks
            .start(Box::new(WaitingTask), harness.hub_tx.clone())
            .unwrap();
        assert!(background_tasks.is_running(&TaskId::AutoFrontlight));
        let levels = LightLevels {
            intensity: 50.0.into(),
            warmth: 25.0.into(),
        };
        let outcome = {
            let mut runtime = DeviceRuntime {
                view: &mut harness.view,
                history: &mut harness.history,
                tasks: &mut harness.tasks,
                updating: &mut harness.updating,
                inactive_since: &mut harness.inactive_since,
                settings_manager: None,
                startup_cwd: None,
                background_tasks: Some(&mut background_tasks),
            };
            handle_event(
                &Event::SetFrontlightLevels(levels),
                &harness.hub_tx,
                &mut harness.bus,
                &mut harness.rq,
                &mut harness.context,
                &mut runtime,
            )
        };
        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(
            harness.context.settings.frontlight_levels.intensity,
            levels.intensity
        );
        assert_eq!(
            harness.context.settings.frontlight_levels.warmth,
            levels.warmth
        );
        assert!(!background_tasks.is_running(&TaskId::AutoFrontlight));
    }
}
