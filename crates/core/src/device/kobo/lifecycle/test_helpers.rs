//! Shared test harness for Kobo lifecycle handler unit tests.
//!
//! [`LifecycleHarness`] builds a minimal [`AppContext`] via
//! [`crate::context::test_helpers::create_test_context`], a root [`Filler`] view,
//! and the [`DeviceRuntime`] fields lifecycle handlers expect (tasks, history,
//! render queue, hub channel). Use it to invoke `handle_event` or private
//! helpers without booting the full application loop.
//!
//! # Example
//!
//! ```ignore
//! let mut harness = LifecycleHarness::new();
//! harness.context.settings.wifi = true;
//! let outcome = harness.with_parts(|hub, bus, rq, context, runtime| {
//!     suspend::handle_event(&Event::PrepareSuspend, hub, rq, context, runtime)
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
use std::time::Instant;

/// Minimal runtime shell for lifecycle handler tests.
///
/// Owns an [`AppContext`], hub channel, view tree, and [`DeviceRuntime`]
/// state. Construct with [`LifecycleHarness::new`] and pass mutable references
/// into handlers via [`LifecycleHarness::with_parts`] or
/// [`LifecycleHarness::with_runtime_only`].
pub struct LifecycleHarness {
    pub context: AppContext,
    pub hub_tx: Hub,
    hub_rx: Receiver<Event>,
    pub bus: Bus,
    pub rq: RenderQueue,
    pub view: Box<dyn View>,
    pub tasks: Vec<DeviceTask>,
    pub history: Vec<HistoryItem>,
    pub updating: Vec<UpdateData>,
    pub inactive_since: Instant,
}

impl LifecycleHarness {
    /// Creates a harness with default test context, empty task list, and root filler view.
    pub fn new() -> Self {
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
            inactive_since: Instant::now(),
        }
    }

    /// Collects all events sent on the hub since the last drain.
    pub fn drain_hub(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.hub_rx.try_recv() {
            events.push(event);
        }
        events
    }

    /// Inserts a placeholder [`DeviceTask`] so handlers see a pending lifecycle task.
    pub fn push_task(&mut self, id: DeviceTaskId) {
        let (_tx, rx) = std::sync::mpsc::channel();
        self.tasks.retain(|task| task.id != id);
        self.tasks.push(DeviceTask { id, _chan: rx });
    }

    /// Runs `f` with hub, bus, render queue, context, and a fresh runtime borrow.
    pub fn with_parts<R>(
        &mut self,
        f: impl FnOnce(&Hub, &mut Bus, &mut RenderQueue, &mut AppContext, &mut DeviceRuntime<'_>) -> R,
    ) -> R {
        let mut runtime = DeviceRuntime {
            view: &mut self.view,
            history: &mut self.history,
            tasks: &mut self.tasks,
            updating: &mut self.updating,
            inactive_since: &mut self.inactive_since,
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
    pub fn with_runtime_only<R>(
        &mut self,
        f: impl FnOnce(&mut AppContext, &mut DeviceRuntime<'_>) -> R,
    ) -> R {
        let mut runtime = DeviceRuntime {
            view: &mut self.view,
            history: &mut self.history,
            tasks: &mut self.tasks,
            updating: &mut self.updating,
            inactive_since: &mut self.inactive_since,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        f(&mut self.context, &mut runtime)
    }
}
