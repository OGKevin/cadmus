//! Rust side of `LibraryPage`.
//!
//! The row model and its callbacks live on [`LibraryPageIo`]. A tap or a
//! long-press passes the model index, starting at 0.

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step, Waiting};
use crate::device::AppContext;
use crate::fl;
use crate::view::slint::generated::{LibraryPageIo, SettingLine, SettingsWindow};
use crate::view::slint::{CallbackQueue, changed_model, connect};
use crate::view::{EntryId, EntryKind};

/// One callback from the library list.
pub(super) enum Action {
    /// `index` is the index into the library model, starting at 0.
    Press(i32),
    /// Long-press on a library row. A negative index is ignored.
    Held(i32),
}

pub(super) struct Libraries {
    queue: CallbackQueue<Action>,
}

impl Libraries {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }

    /// Builds the delete-row choice, or [`Step::Idle`] when deletion is not
    /// offered. The last remaining library is not deletable.
    pub(super) fn hold(&self, row: i32, context: &AppContext) -> Step {
        if row < 0 {
            return Step::Idle;
        }
        let index = row as usize;
        let Some(library) = context.settings.libraries.get(index) else {
            return Step::Idle;
        };
        if context.settings.libraries.len() <= 1 {
            return Step::Idle;
        }
        Step::choice(
            Waiting::DeleteLibrary(index),
            library.name.clone(),
            vec![EntryKind::Command(
                fl!("delete"),
                EntryId::DeleteLibrary(index),
            )],
        )
    }
}

#[async_trait::async_trait(?Send)]
impl Component for Libraries {
    type Window = SettingsWindow;
    type Action = Action;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<LibraryPageIo>();
        connect!(io, self.queue, on_row_pressed, (index) => Action::Press(index));
        connect!(io, self.queue, on_row_held, (index) => Action::Held(index));
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<LibraryPageIo>();
        if let Some(rows) = changed_model(&io.get_rows(), library_lines(context)) {
            io.set_rows(rows);
        }
    }

    async fn apply(&mut self, action: Action, env: &mut Env<'_>) -> Step {
        match action {
            Action::Press(index) => setting::activate(PressedRow::Library(index), env),
            Action::Held(row) => self.hold(row, env.context),
        }
    }
}

/// Removes library `index` and keeps the settings invariant.
pub(super) fn delete(index: usize, context: &mut AppContext) {
    if index < context.settings.libraries.len() {
        context.settings.libraries.remove(index);
        context.settings.sanitize_libraries();
        if context.settings.selected_library >= context.settings.libraries.len() {
            context.settings.selected_library = context.settings.libraries.len() - 1;
        }
    }
}

fn library_lines(context: &AppContext) -> Vec<SettingLine> {
    context
        .settings
        .libraries
        .iter()
        .map(|library| SettingLine {
            label: library.name.clone().into(),
            value: library.path.display().to_string().into(),
        })
        .collect()
}
