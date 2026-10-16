//! Download progress: label, bar, cancel.

use crate::device::AppContext;
use crate::view::slint::generated::OtaWindow;
use crate::view::slint::{CallbackQueue, Component, Env, connect};

pub(super) struct Progress {
    queue: CallbackQueue<()>,
}

impl Progress {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn publish(
        &self,
        window: &OtaWindow,
        label: &str,
        percent: u8,
        indeterminate: bool,
        cancelable: bool,
    ) {
        window.set_progress_label(label.into());
        window.set_progress(f32::from(percent) / 100.0);
        window.set_progress_indeterminate(indeterminate);
        window.set_progress_cancelable(cancelable);
    }
}

#[async_trait::async_trait(?Send)]
impl Component for Progress {
    type Window = OtaWindow;
    type Action = ();
    type Effect = ();

    fn wire(&self, window: &OtaWindow) {
        connect!(window, self.queue, on_progress_cancel, () => ());
    }

    fn take(&self) -> Vec<()> {
        self.queue.drain()
    }

    fn sync(&mut self, _window: &OtaWindow, _context: &AppContext) {}

    async fn apply(&mut self, (): (), _env: &mut Env<'_>) {}
}
