//! Sign-in panel: message, code, cancel.

use crate::device::AppContext;
use crate::view::slint::generated::OtaWindow;
use crate::view::slint::{CallbackQueue, Component, Env, connect};

pub(super) struct AuthPanel {
    queue: CallbackQueue<()>,
}

impl AuthPanel {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn publish(&self, window: &OtaWindow, message: &str, code: &str) {
        window.set_auth_message(message.into());
        window.set_auth_code(code.into());
    }
}

#[async_trait::async_trait(?Send)]
impl Component for AuthPanel {
    type Window = OtaWindow;
    type Action = ();
    type Effect = ();

    fn wire(&self, window: &OtaWindow) {
        connect!(window, self.queue, on_auth_cancel, () => ());
    }

    fn take(&self) -> Vec<()> {
        self.queue.drain()
    }

    fn sync(&mut self, _window: &OtaWindow, _context: &AppContext) {}

    async fn apply(&mut self, (): (), _env: &mut Env<'_>) {}
}
