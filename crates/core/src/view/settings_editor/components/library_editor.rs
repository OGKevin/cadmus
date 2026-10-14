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
            name: fl!("untitled"),
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
                    (Event::Notification(NotificationEvent::Show(fl!(
                        "notification-library-name-empty"
                    ))))
                    .into(),
                )
                .ok();
            return Step::Idle;
        }
        if tokio::fs::metadata(&draft.library.path).await.is_err() {
            env.hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl!(
                        "notification-library-path-missing"
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
        io.set_name_label(fl!("settings-library-name").into());
        io.set_path_label(fl!("settings-library-path").into());
        io.set_finished_label(fl!("settings-library-end-of-book-action").into());
        io.set_accept_label(fl!("ok").into());
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use slint::ComponentHandle;

    use super::super::notification;
    use super::super::settings_env;
    use super::super::text_limits;
    use super::super::{FilesMode, Sheet, Waiting};
    use super::{Action, Component, LibraryEditor, Step};
    use crate::fl;
    use crate::i18n::I18nDisplay;
    use crate::settings::{FinishedAction, LibrarySettings};
    use crate::view::slint::generated::LibraryEditorIo;
    use crate::view::{EntryId, EntryKind};

    fn checked(entries: &[EntryKind], id: &EntryId) -> bool {
        entries.iter().any(|entry| match entry {
            EntryKind::RadioButton(_, entry_id, on) => entry_id == id && *on,
            _ => false,
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_shows_the_draft_and_callbacks_open_its_fields() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let dir = tempfile::tempdir().expect("library dir");
        context.settings.libraries = vec![LibrarySettings {
            name: "KePub".into(),
            path: dir.path().to_path_buf(),
            finished: Some(FinishedAction::Close),
            ..LibrarySettings::default()
        }];

        let mut editor = LibraryEditor::new();
        editor.wire(window);
        editor.open(Some(0), &context);
        editor.sync(window, &context);

        let io = window.global::<LibraryEditorIo>();
        assert!(io.get_open());
        assert_eq!(io.get_heading().as_str(), "KePub");
        assert_eq!(io.get_library_name().as_str(), "KePub");
        assert_eq!(io.get_path().as_str(), dir.path().display().to_string());
        assert_eq!(
            io.get_finished().as_str(),
            FinishedAction::Close.to_i18n_string()
        );
        assert_eq!(io.get_name_label().as_str(), fl!("settings-library-name"));

        io.invoke_name_pressed();
        io.invoke_path_pressed();
        io.invoke_finished_pressed();
        assert!(matches!(
            editor.take().as_slice(),
            [
                Action::NamePressed,
                Action::PathPressed,
                Action::FinishedPressed
            ]
        ));

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let name = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::NamePressed, &mut env).await
        };
        let Step::Ask(
            Waiting::LibraryName,
            Sheet::Text {
                label,
                initial,
                limits,
            },
        ) = name
        else {
            panic!("name press did not open the name field");
        };
        assert_eq!(label, fl!("settings-library-name"));
        assert_eq!(initial, "KePub");
        assert_eq!(limits, text_limits::LIBRARY_NAME);

        let path = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::PathPressed, &mut env).await
        };
        let Step::Ask(Waiting::LibraryPath, Sheet::File { start, mode }) = path else {
            panic!("path press did not open the directory browser");
        };
        assert_eq!(start, dir.path());
        assert_eq!(mode, FilesMode::Directory);

        editor.apply_finished(&EntryId::SetLibraryFinishedAction(
            1,
            FinishedAction::GoToNext,
        ));
        editor.sync(window, &context);
        assert_eq!(
            window.global::<LibraryEditorIo>().get_finished().as_str(),
            FinishedAction::Close.to_i18n_string(),
            "a finished action for another library is ignored"
        );
        editor.apply_finished(&EntryId::SetLibraryFinishedAction(
            0,
            FinishedAction::Notify,
        ));
        editor.sync(window, &context);
        assert_eq!(
            window.global::<LibraryEditorIo>().get_finished().as_str(),
            FinishedAction::Notify.to_i18n_string()
        );

        let finished = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::FinishedPressed, &mut env).await
        };
        let Step::Ask(Waiting::LibraryFinished, Sheet::Choice { entries, .. }) = finished else {
            panic!("finished press did not open the action list");
        };
        assert_eq!(entries.len(), 4);
        assert!(checked(
            &entries,
            &EntryId::SetLibraryFinishedAction(0, FinishedAction::Notify)
        ));
        assert!(!checked(&entries, &EntryId::ClearLibraryFinishedAction(0)));

        editor.apply_finished(&EntryId::ClearLibraryFinishedAction(0));
        window.global::<LibraryEditorIo>().invoke_dismissed();
        assert!(matches!(editor.take().as_slice(), [Action::Dismissed]));
        let dismissed = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::Dismissed, &mut env).await
        };
        assert!(matches!(dismissed, Step::Redraw));
        assert!(!editor.is_open());
        editor.sync(window, &context);
        assert!(!window.global::<LibraryEditorIo>().get_open());
        assert_eq!(
            context.settings.libraries[0].finished,
            Some(FinishedAction::Close)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_stores_a_library_only_when_the_name_and_path_exist() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        context.settings.libraries.clear();

        let mut editor = LibraryEditor::new();
        editor.wire(window);
        editor.open(Some(0), &context);
        assert!(!editor.is_open());
        editor.set_name("ignored".into());
        assert!(!editor.is_open());

        editor.open(None, &context);
        editor.sync(window, &context);
        let io = window.global::<LibraryEditorIo>();
        assert!(io.get_open());
        assert_eq!(io.get_library_name().as_str(), fl!("untitled"));
        assert_eq!(io.get_path().as_str(), "/mnt/onboard");

        let max = text_limits::LIBRARY_NAME.max_chars;
        let kept = "a".repeat(max);
        editor.set_name(format!("{kept}z"));
        editor.sync(window, &context);
        assert_eq!(
            window
                .global::<LibraryEditorIo>()
                .get_library_name()
                .as_str(),
            kept
        );

        let (hub, mut rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        editor.set_name("   ".into());
        let empty = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::Accepted, &mut env).await
        };
        assert!(matches!(empty, Step::Idle));
        assert!(editor.is_open());
        assert!(context.settings.libraries.is_empty());
        assert_eq!(
            notification(&mut rx),
            fl!("notification-library-name-empty")
        );

        editor.set_name("Books".into());
        editor.set_path(PathBuf::from("/no/such/cadmus/library"));
        let missing = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::Accepted, &mut env).await
        };
        assert!(matches!(missing, Step::Idle));
        assert!(editor.is_open());
        assert!(context.settings.libraries.is_empty());
        assert_eq!(
            notification(&mut rx),
            fl!("notification-library-path-missing")
        );

        let dir = tempfile::tempdir().expect("library dir");
        let file = tempfile::NamedTempFile::new().expect("library file");
        editor.set_path(file.path().to_path_buf());
        let file_parent = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::PathPressed, &mut env).await
        };
        let Step::Ask(Waiting::LibraryPath, Sheet::File { start, mode }) = file_parent else {
            panic!("a file path did not browse from its parent");
        };
        assert_eq!(mode, FilesMode::Directory);
        assert_eq!(start, file.path().parent().expect("file parent"));

        editor.set_path(dir.path().to_path_buf());
        window.global::<LibraryEditorIo>().invoke_accepted();
        assert!(matches!(editor.take().as_slice(), [Action::Accepted]));
        let stored = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::Accepted, &mut env).await
        };
        assert!(matches!(stored, Step::Redraw));
        assert!(!editor.is_open());
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Books");
        assert_eq!(context.settings.libraries[0].path, dir.path());
        editor.sync(window, &context);
        assert!(!window.global::<LibraryEditorIo>().get_open());

        editor.open(Some(0), &context);
        editor.set_name("Shelf".into());
        let renamed = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            editor.apply(Action::Accepted, &mut env).await
        };
        assert!(matches!(renamed, Step::Redraw));
        assert_eq!(context.settings.libraries.len(), 1);
        assert_eq!(context.settings.libraries[0].name, "Shelf");
        assert_eq!(context.settings.libraries[0].path, dir.path());
    }
}
