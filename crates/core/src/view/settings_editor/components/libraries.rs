//! Rust side of `LibraryPage`.
//!
//! The row model and its callbacks live on [`LibraryPageIo`]. A tap or a
//! long-press passes the model index, starting at 0.

use slint::ComponentHandle;

use super::setting::{self, PressedRow};
use super::{Component, Env, Step, Waiting};
use crate::device::AppContext;
use crate::fl;
use crate::view::slint::generated::{LibraryPageIo, PagedLine, SettingsWindow};
use crate::view::slint::{CallbackQueue, ListRow, changed_model, connect, paged_line};
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

/// Removes library `index`.
///
/// An earlier removal shifts the open library left, so its index drops by one.
/// Removing the open library leaves the index on the entry that slides into
/// that slot. The index is then clamped to the last remaining library. This
/// function does not reload that library.
pub(super) fn delete(index: usize, context: &mut AppContext) {
    if index >= context.settings.libraries.len() {
        return;
    }
    context.settings.libraries.remove(index);
    if index < context.settings.selected_library {
        context.settings.selected_library -= 1;
    }
    context.settings.sanitize_libraries();
    if context.settings.selected_library >= context.settings.libraries.len() {
        context.settings.selected_library = context.settings.libraries.len() - 1;
    }
}

fn library_lines(context: &AppContext) -> Vec<PagedLine> {
    context
        .settings
        .libraries
        .iter()
        .map(|library| {
            paged_line(ListRow {
                label: library.name.clone(),
                value: library.path.display().to_string(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::delete;
    use crate::context::test_helpers::create_test_context;
    use crate::settings::LibrarySettings;
    use std::path::PathBuf;

    fn library(name: &str) -> LibrarySettings {
        LibrarySettings {
            name: name.to_string(),
            path: PathBuf::from(name),
            ..LibrarySettings::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_earlier_library_keeps_index_on_open_library() {
        let mut context = create_test_context().await;
        context.settings.libraries = vec![library("A"), library("B"), library("C")];
        context.settings.selected_library = 1;

        delete(0, &mut context);

        assert_eq!(context.settings.selected_library, 0);
        assert_eq!(context.settings.libraries[0].name, "B");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_selected_library_indexes_the_next_library() {
        let mut context = create_test_context().await;
        context.settings.libraries = vec![library("A"), library("B"), library("C")];
        context.settings.selected_library = 1;

        delete(1, &mut context);

        assert_eq!(context.settings.selected_library, 1);
        assert_eq!(context.settings.libraries[1].name, "C");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delete_last_selected_library_clamps_to_new_last() {
        let mut context = create_test_context().await;
        context.settings.libraries = vec![library("A"), library("B"), library("C")];
        context.settings.selected_library = 2;

        delete(2, &mut context);

        assert_eq!(context.settings.selected_library, 1);
        assert_eq!(context.settings.libraries[1].name, "B");
    }
}
