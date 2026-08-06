//! Shared test harness for device handler unit tests.
//!
//! [`DeviceRuntimeHarness`] builds a minimal [`AppContext`] via
//! [`crate::context::test_helpers::create_test_context`], a root [`Filler`] view,
//! and the [`DeviceRuntime`] fields handlers expect (tasks, history, render
//! queue, hub channel). Use it without booting the full application loop.
//!
//! # Example
//!
//! ```ignore
//! let mut harness = DeviceRuntimeHarness::new();
//! harness.context.settings.wifi = WifiMode::AlwaysOn;
//! let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
//!     suspend::handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
//! });
//! assert_eq!(outcome, EventOutcome::Handled);
//! ```

use crate::color::WHITE;
use crate::context::test_helpers::create_test_context;
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::{DeviceRuntime, DeviceTask, DeviceTaskId, HistoryItem};
use crate::framebuffer::Framebuffer as _;
use crate::view::filler::Filler;
use crate::view::{Bus, Event, Hub, RenderQueue, UpdateData, View};
use std::sync::mpsc::Receiver;

/// Minimal runtime shell for device / suspend handler tests.
///
/// Owns an [`AppContext`], hub channel, view tree, and [`DeviceRuntime`]
/// state. Construct with [`DeviceRuntimeHarness::new`] and pass mutable
/// references into handlers via [`DeviceRuntimeHarness::with_parts`] or
/// [`DeviceRuntimeHarness::with_runtime_only`].
pub(crate) struct DeviceRuntimeHarness {
    pub(crate) context: AppContext,
    pub(crate) hub_tx: Hub,
    hub_rx: Receiver<Event>,
    pub(crate) bus: Bus,
    pub(crate) rq: RenderQueue,
    pub(crate) view: Box<dyn View>,
    pub(crate) tasks: Vec<DeviceTask>,
    pub(crate) history: Vec<HistoryItem>,
    pub(crate) updating: Vec<UpdateData>,
}

impl DeviceRuntimeHarness {
    /// Creates a harness with default test context, empty task list, and root filler view.
    pub(crate) fn new() -> Self {
        let (hub_tx, hub_rx) = std::sync::mpsc::channel();
        let context = create_test_context();
        let rect = context.device.framebuffer().rect();
        let view: Box<dyn View> = Box::new(Filler::new(rect, WHITE));
        Self {
            context,
            hub_tx,
            hub_rx,
            bus: Bus::new(),
            rq: RenderQueue::new(),
            view,
            tasks: Vec::new(),
            history: Vec::new(),
            updating: Vec::new(),
        }
    }

    /// Collects all events sent on the hub since the last drain.
    pub(crate) fn drain_hub(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.hub_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Inserts a placeholder [`DeviceTask`] so handlers see a pending device task.
    pub(crate) fn push_task(&mut self, id: DeviceTaskId) {
        let (_tx, rx) = std::sync::mpsc::channel();
        self.tasks.retain(|task| task.id != id);
        self.tasks.push(DeviceTask { id, _chan: rx });
    }

    /// Runs `f` with hub, bus, render queue, context, and a fresh runtime borrow.
    pub(crate) fn with_parts<R>(
        &mut self,
        f: impl FnOnce(&Hub, &mut Bus, &mut RenderQueue, &mut AppContext, &mut DeviceRuntime<'_>) -> R,
    ) -> R {
        let mut runtime = DeviceRuntime {
            view: &mut self.view,
            history: &mut self.history,
            tasks: &mut self.tasks,
            updating: &mut self.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        f(
            &self.hub_tx,
            &mut self.bus,
            &mut self.rq,
            &mut self.context,
            &mut runtime,
        )
    }

    /// Runs `f` with only context and runtime when bus/render queue are unused.
    pub(crate) fn with_runtime_only<R>(
        &mut self,
        f: impl FnOnce(&mut AppContext, &mut DeviceRuntime<'_>) -> R,
    ) -> R {
        let mut runtime = DeviceRuntime {
            view: &mut self.view,
            history: &mut self.history,
            tasks: &mut self.tasks,
            updating: &mut self.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        f(&mut self.context, &mut runtime)
    }
}
