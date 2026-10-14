//! Shared confirmation dialog behind `ConfirmSheet`.
//!
//! The component (`ui/widgets/confirm_sheet.slint`) draws a centered card with
//! the message and the cancel/accept pair. A tap outside the card dismisses
//! it, and the page underneath stays. The type parameter is the window that
//! draws it: any window whose component exports `ConfirmSheetIo`. A screen
//! constructs `ConfirmSheet<ItsWindow>` and remembers why it opened the dialog.

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::fl;
use crate::view::slint::component::{Component, Env};
use crate::view::slint::generated::ConfirmSheetIo;
use crate::view::slint::{CallbackQueue, connect};

/// One callback from the confirmation dialog.
#[derive(Debug)]
pub(in crate::view) enum Action {
    Accepted,
    Dismissed,
}

/// What the screen does after one callback.
pub(in crate::view) enum Outcome {
    /// Nothing further.
    Idle,
    /// The reader closed the dialog.
    Dismissed,
    /// The reader accepted. The dialog is closed.
    Accepted,
}

/// State of one open confirmation dialog.
pub(in crate::view) struct ConfirmSheet<W> {
    pending: Option<Pending>,
    queue: CallbackQueue<Action>,
    window: super::WindowOf<W>,
}

struct Pending {
    message: String,
    accept: String,
}

impl<W> ConfirmSheet<W> {
    pub(in crate::view) fn new() -> Self {
        Self {
            pending: None,
            queue: CallbackQueue::new(),
            window: super::WindowOf::default(),
        }
    }

    /// Opens the dialog on `message`, accepting with `accept`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "confirm_sheet.open", skip_all)
    )]
    pub(in crate::view) fn open(&mut self, message: String, accept: String) {
        tracing::debug!("confirmation opened");
        self.pending = Some(Pending { message, accept });
    }

    /// Closes the dialog.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "confirm_sheet.close", skip_all)
    )]
    pub(in crate::view) fn close(&mut self) {
        if self.pending.is_none() {
            return;
        }
        tracing::debug!("confirmation closed");
        self.pending = None;
    }

    pub(in crate::view) fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    fn handle(&mut self, action: Action) -> Outcome {
        match action {
            Action::Accepted => match self.pending.take() {
                Some(_) => {
                    tracing::debug!("confirmation accepted");
                    Outcome::Accepted
                }
                None => Outcome::Idle,
            },
            Action::Dismissed => {
                self.close();
                Outcome::Dismissed
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<W> Component for ConfirmSheet<W>
where
    W: ComponentHandle + 'static,
    for<'a> ConfirmSheetIo<'a>: Global<'a, W>,
{
    type Window = W;
    type Action = Action;
    type Effect = Outcome;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "confirm_sheet.wire", skip_all)
    )]
    fn wire(&self, window: &W) {
        let io = window.global::<ConfirmSheetIo>();
        connect!(io, self.queue, on_accepted, () => Action::Accepted);
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "confirm_sheet.sync",
            level = "trace",
            skip_all,
            fields(open = self.pending.is_some())
        )
    )]
    fn sync(&mut self, window: &W, _context: &AppContext) {
        let io = window.global::<ConfirmSheetIo>();
        let Some(pending) = &self.pending else {
            io.set_open(false);
            return;
        };
        io.set_message(pending.message.as_str().into());
        io.set_cancel_label(fl!("cancel").into());
        io.set_accept_label(pending.accept.as_str().into());
        io.set_open(true);
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "confirm_sheet.apply", skip_all, fields(action = ?action))
    )]
    async fn apply(&mut self, action: Action, _env: &mut Env<'_>) -> Outcome {
        self.handle(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepting_closes_the_dialog() {
        let mut sheet = ConfirmSheet::<()>::new();
        assert!(matches!(sheet.handle(Action::Accepted), Outcome::Idle));
        sheet.open("Import every book?".into(), "Import".into());
        assert!(matches!(sheet.handle(Action::Accepted), Outcome::Accepted));
        assert!(!sheet.is_open());
    }

    #[test]
    fn dismissing_closes_the_dialog() {
        let mut sheet = ConfirmSheet::<()>::new();
        sheet.open("Import every book?".into(), "Import".into());
        assert!(matches!(
            sheet.handle(Action::Dismissed),
            Outcome::Dismissed
        ));
        assert!(!sheet.is_open());
    }
}
