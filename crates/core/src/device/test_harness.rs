//! Shared test harness for device handler unit tests.
//!
//! [`DeviceRuntimeHarness`] builds a minimal [`AppContext`] via
//! [`crate::context::test_helpers::create_test_context`], a root [`Filler`] view,
//! and the [`DeviceRuntime`] fields handlers expect (tasks, history, render
//! queue, hub channel). Use it without booting the full application loop.
//!
//! Handler tests are `#[tokio::test]` async functions. Use [`crate::poll_parts!`]
//! or [`crate::poll_runtime_only!`] to `.await` an async handler, and
//! [`DeviceRuntimeHarness::with_parts`] /
//! [`DeviceRuntimeHarness::with_runtime_only`] for synchronous handlers.
//! The polling macros expand at the call site, so the [`DeviceRuntime`] they
//! build and the future borrowing it share one scope.
//! [`DeviceRuntimeHarness::run_on_shutdown`] is async — always `.await` it.
//!
//! # Example
//!
//! ```ignore
//! let mut harness = DeviceRuntimeHarness::new().await;
//! harness.context.settings.wifi = WifiMode::AlwaysOn;
//! let outcome = crate::poll_parts!(harness, |hub, bus, rq, context, runtime| {
//!     suspend::handle_event(&Event::PrepareSuspend, hub, bus, rq, context, runtime)
//! });
//! assert_eq!(outcome, EventOutcome::Handled);
//! ```

use crate::color::WHITE;
use crate::context::test_helpers::create_test_context;
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::{
    DeviceLifecycle, DeviceRuntime, DeviceTask, DeviceTaskId, ExitStatus, HistoryItem,
};
use crate::framebuffer::Framebuffer as _;
use crate::view::filler::Filler;
use crate::view::{Bus, Event, Hub, RenderQueue, UpdateData, View};

/// Minimal runtime shell for device / suspend handler tests.
///
/// Owns an [`AppContext`], hub channel, view tree, and [`DeviceRuntime`]
/// state. Construct with [`DeviceRuntimeHarness::new`] and pass mutable
/// references into handlers via [`crate::poll_parts!`], [`crate::poll_runtime_only!`], or the
/// `_sync` helpers.
pub(crate) struct DeviceRuntimeHarness {
    pub(crate) context: AppContext,
    pub(crate) hub_tx: Hub,
    hub_rx: crate::view::HubReceiver,
    pub(crate) bus: Bus,
    pub(crate) rq: RenderQueue,
    pub(crate) view: Box<dyn View>,
    pub(crate) tasks: Vec<DeviceTask>,
    pub(crate) history: Vec<HistoryItem>,
    pub(crate) updating: Vec<UpdateData>,
}

// Not every device feature compiles every test that uses this harness, so a
// given feature build sees helpers it never calls. `poll_parts!` and friends
// cover the async paths; the rest are for the feature's own tests.
#[allow(dead_code)]
impl DeviceRuntimeHarness {
    /// Creates a harness with default test context, empty task list, and root filler view.
    pub(crate) async fn new() -> Self {
        let (hub_tx, hub_rx) = crate::view::hub_channel();
        let context = create_test_context().await;
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
    pub(crate) fn drain_hub(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(message) = self.hub_rx.try_recv() {
            events.push(message.event);
        }
        events
    }

    /// Inserts a placeholder [`DeviceTask`] so handlers see a pending device task.
    pub(crate) fn push_task(&mut self, id: DeviceTaskId) {
        self.tasks.retain(|task| task.id != id);
        self.tasks.push(DeviceTask {
            id,
            job: crate::runtime::Job::spawn(|_| std::future::pending()),
        });
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

    /// Runs [`DeviceLifecycle::on_shutdown`] with optional pre-shutdown setup.
    pub(crate) async fn run_on_shutdown<D: DeviceLifecycle>(
        &mut self,
        status: ExitStatus,
        prep: impl FnOnce(&mut AppContext),
    ) {
        prep(&mut self.context);
        let mut shutdown = self.context.shutdown();
        let tasks = std::mem::take(&mut self.tasks);
        D::on_shutdown(&mut shutdown, status, &tasks)
            .await
            .expect("on_shutdown in test harness");
        self.tasks = tasks;
    }
}

/// Awaits an async handler with hub, bus, render queue, context, and a fresh
/// runtime borrow.
///
/// Expands at the call site so the [`DeviceRuntime`] local and the handler
/// future that borrows it share one scope, which is what lets the future be
/// `.await`ed directly instead of driven from a synchronous frame.
#[macro_export]
macro_rules! poll_parts {
    ($harness:expr, |$hub:ident, $bus:ident, $rq:ident, $context:ident, $runtime:ident| $future:expr) => {
        $crate::__poll_parts!($harness, None, |$hub, $bus, $rq, $context, $runtime| {
            $future
        })
    };
}

/// Like [`poll_parts`], but lends a [`TaskManager`](crate::task::TaskManager) to
/// the handler as `runtime.background_tasks`.
#[macro_export]
macro_rules! poll_parts_with_background {
    ($harness:expr, $background:expr, |$hub:ident, $bus:ident, $rq:ident, $context:ident, $runtime:ident| $future:expr) => {
        $crate::__poll_parts!(
            $harness,
            Some($background),
            |$hub, $bus, $rq, $context, $runtime| $future
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __poll_parts {
    ($harness:expr, $background:expr, |$hub:ident, $bus:ident, $rq:ident, $context:ident, $runtime:ident| $future:expr) => {{
        let mut runtime = $crate::device::DeviceRuntime {
            view: &mut $harness.view,
            history: &mut $harness.history,
            tasks: &mut $harness.tasks,
            updating: &mut $harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: $background,
        };
        let ($hub, $bus, $rq, $context, $runtime) = (
            &$harness.hub_tx,
            &mut $harness.bus,
            &mut $harness.rq,
            &mut $harness.context,
            &mut runtime,
        );
        $future.await
    }};
}

/// Awaits an async handler with only context and a fresh runtime borrow.
#[macro_export]
macro_rules! poll_runtime_only {
    ($harness:expr, |$context:ident, $runtime:ident| $future:expr) => {{
        let mut runtime = $crate::device::DeviceRuntime {
            view: &mut $harness.view,
            history: &mut $harness.history,
            tasks: &mut $harness.tasks,
            updating: &mut $harness.updating,
            settings_manager: None,
            startup_cwd: None,
            background_tasks: None,
        };
        let ($context, $runtime) = (&mut $harness.context, &mut runtime);
        $future.await
    }};
}
