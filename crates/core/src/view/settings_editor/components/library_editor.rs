//! Rust side of `LibraryEditor`.
//!
//! The heading, name, path, finished action, and callbacks live on
//! [`LibraryEditorIo`].

use std::path::{Path, PathBuf};

use slint::ComponentHandle;

use super::text_limits;
use super::{Component, Env, FilesMode, Step, Waiting};
use crate::device::AppContext;
use crate::fl;
use crate::fl_or;
use crate::i18n::I18nDisplay;
use crate::settings::{FinishedAction, LibrarySettings};
use crate::view::slint::generated::{LibraryEditorIo, SettingsWindow};
use crate::view::slint::{CallbackQueue, connect};
use crate::view::{EntryId, EntryKind, Event, NotificationEvent};

/// One callback from the library editor.
pub(super) enum Action {
    NamePressed,
    PathPressed,
    FinishedPressed,
    Accepted,
    Dismissed,
}

#[derive(Clone)]
struct Draft {
    index: usize,
    library: LibrarySettings,
}

pub(super) struct LibraryEditor {
    draft: Option<Draft>,
    queue: CallbackQueue<Action>,
}

impl LibraryEditor {
    pub(super) fn new() -> Self {
        Self {
            draft: None,
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn close(&mut self) {
        self.draft = None;
    }

    pub(super) fn open(&mut self, index: Option<usize>, context: &AppContext) {
        match index {
            Some(index) => self.open_existing(index, context),
            None => self.open_new(context),
        }
    }

    fn open_existing(&mut self, index: usize, context: &AppContext) {
        let Some(library) = context.settings.libraries.get(index).cloned() else {
            return;
        };
        self.draft = Some(Draft { index, library });
    }

    fn open_new(&mut self, context: &AppContext) {
        let library = LibrarySettings {
            name: "untitled".to_string(),
            path: PathBuf::from("/mnt/onboard"),
            ..Default::default()
        };
        self.draft = Some(Draft {
            index: context.settings.libraries.len(),
            library,
        });
    }

    fn name(&self) -> Option<String> {
        self.draft.as_ref().map(|draft| draft.library.name.clone())
    }

    fn path(&self) -> Option<PathBuf> {
        self.draft.as_ref().map(|draft| draft.library.path.clone())
    }

    pub(super) fn set_name(&mut self, text: String) {
        if let Some(draft) = &mut self.draft {
            draft.library.name = text
                .chars()
                .take(text_limits::LIBRARY_NAME.max_chars)
                .collect();
        }
    }

    pub(super) fn set_path(&mut self, path: PathBuf) {
        if let Some(draft) = &mut self.draft {
            draft.library.path = path;
        }
    }

    pub(super) fn apply_finished(&mut self, id: &EntryId) {
        let Some(draft) = &mut self.draft else {
            return;
        };
        match id {
            EntryId::ClearLibraryFinishedAction(index) if *index == draft.index => {
                draft.library.finished = None;
            }
            EntryId::SetLibraryFinishedAction(index, action) if *index == draft.index => {
                draft.library.finished = Some(*action);
            }
            _ => {}
        }
    }

    async fn accept(&mut self, env: &mut Env<'_>) -> Step {
        let Some(draft) = self.draft.clone() else {
            return Step::Idle;
        };
        if draft.library.name.trim().is_empty() {
            env.hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl_or!(
                        "notification-library-name-empty",
                        "Library name cannot be empty"
                    ))))
                    .into(),
                )
                .ok();
            return Step::Idle;
        }
        if tokio::fs::metadata(&draft.library.path).await.is_err() {
            env.hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl_or!(
                        "notification-library-path-missing",
                        "Path does not exist"
                    ))))
                    .into(),
                )
                .ok();
            return Step::Idle;
        }
        let index = draft.index;
        if index < env.context.settings.libraries.len() {
            env.context.settings.libraries[index] = draft.library;
        } else if index == env.context.settings.libraries.len() {
            env.context.settings.libraries.push(draft.library);
        }
        self.draft = None;
        Step::Redraw
    }

    /// Directory the file browser should open for this library's path.
    async fn browse_start(&self) -> Option<PathBuf> {
        let path = self.path()?;
        if tokio::fs::metadata(&path)
            .await
            .map(|meta| meta.is_dir())
            .unwrap_or(false)
        {
            Some(path)
        } else {
            Some(
                path.parent()
                    .map(Path::to_path_buf)
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| PathBuf::from("/")),
            )
        }
    }

    fn edit_finished(&self) -> Option<(usize, Option<FinishedAction>)> {
        self.draft
            .as_ref()
            .map(|draft| (draft.index, draft.library.finished))
    }
}

#[async_trait::async_trait(?Send)]
impl Component for LibraryEditor {
    type Window = SettingsWindow;
    type Action = Action;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<LibraryEditorIo>();
        connect!(io, self.queue, on_name_pressed, () => Action::NamePressed);
        connect!(io, self.queue, on_path_pressed, () => Action::PathPressed);
        connect!(io, self.queue, on_finished_pressed, () => Action::FinishedPressed);
        connect!(io, self.queue, on_accepted, () => Action::Accepted);
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, _context: &AppContext) {
        let io = window.global::<LibraryEditorIo>();
        let open = self.draft.is_some();
        let heading = self
            .draft
            .as_ref()
            .map(|draft| draft.library.name.clone())
            .unwrap_or_default();
        let path = self
            .draft
            .as_ref()
            .map(|draft| draft.library.path.display().to_string())
            .unwrap_or_default();
        let finished = self
            .draft
            .as_ref()
            .map(|draft| finished_label(draft.library.finished))
            .unwrap_or_default();
        io.set_open(open);
        io.set_heading(heading.clone().into());
        io.set_library_name(heading.into());
        io.set_path(path.into());
        io.set_finished(finished.into());
    }

    async fn apply(&mut self, action: Action, env: &mut Env<'_>) -> Step {
        match action {
            Action::NamePressed => Step::text(
                Waiting::LibraryName,
                fl!("settings-library-name"),
                self.name().unwrap_or_default(),
                text_limits::LIBRARY_NAME,
            ),
            Action::PathPressed => match self.browse_start().await {
                Some(start) => Step::file(Waiting::LibraryPath, start, FilesMode::Directory),
                None => Step::Idle,
            },
            Action::FinishedPressed => match self.edit_finished() {
                Some((index, current)) => Step::choice(
                    Waiting::LibraryFinished,
                    fl!("settings-library-end-of-book-action"),
                    finished_entries(index, current),
                ),
                None => Step::Idle,
            },
            Action::Accepted => self.accept(env).await,
            Action::Dismissed => {
                self.draft = None;
                Step::Redraw
            }
        }
    }
}

fn finished_entries(index: usize, current: Option<FinishedAction>) -> Vec<EntryKind> {
    vec![
        EntryKind::RadioButton(
            fl!("settings-library-inherit"),
            EntryId::ClearLibraryFinishedAction(index),
            current.is_none(),
        ),
        EntryKind::RadioButton(
            FinishedAction::Notify.to_i18n_string(),
            EntryId::SetLibraryFinishedAction(index, FinishedAction::Notify),
            current == Some(FinishedAction::Notify),
        ),
        EntryKind::RadioButton(
            FinishedAction::Close.to_i18n_string(),
            EntryId::SetLibraryFinishedAction(index, FinishedAction::Close),
            current == Some(FinishedAction::Close),
        ),
        EntryKind::RadioButton(
            FinishedAction::GoToNext.to_i18n_string(),
            EntryId::SetLibraryFinishedAction(index, FinishedAction::GoToNext),
            current == Some(FinishedAction::GoToNext),
        ),
    ]
}

fn finished_label(action: Option<FinishedAction>) -> String {
    action
        .map(|action| action.to_i18n_string())
        .unwrap_or_else(|| fl!("settings-library-inherit"))
}
