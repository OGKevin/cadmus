//! Rust side of `RefreshRates`.
//!
//! The regular and inverted values, the kind list, the per-kind editor, and
//! the callbacks live on [`RefreshRatesIo`]. Kind indexes start at 0.

use std::collections::HashSet;

use slint::ComponentHandle;

use super::text_limits;
use super::{Component, Env, Step, Waiting};
use crate::device::AppContext;
use crate::document::file_extension::FileExtension;
use crate::fl;
use crate::settings::RefreshRatePair;
use crate::view::settings_editor::kinds::SettingKind;
use crate::view::settings_editor::kinds::reader::{
    RefreshRateInvertedSetting, RefreshRateRegularSetting,
};
use crate::view::slint::generated::{PagedLine, RefreshRatesIo, SettingsWindow};
use crate::view::slint::{CallbackQueue, changed_model, connect};
use crate::view::{Bus, EntryId, EntryKind, Event, ViewId};

/// One callback from the refresh-rate editor.
pub(super) enum Action {
    Dismissed,
    Regular,
    Inverted,
    KindPressed(usize),
    KindHeld(usize),
    Add,
    KindDismissed,
    KindAccepted,
    KindRegularPressed,
    KindInvertedPressed,
}

struct KindDraft {
    ext: FileExtension,
    pair: RefreshRatePair,
}

pub(super) struct RefreshRates {
    open: bool,
    kind: Option<KindDraft>,
    queue: CallbackQueue<Action>,
}

impl RefreshRates {
    pub(super) fn new() -> Self {
        Self {
            open: false,
            kind: None,
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn kind_is_open(&self) -> bool {
        self.kind.is_some()
    }

    pub(super) fn close_kind(&mut self) {
        self.kind = None;
    }

    pub(super) fn close(&mut self) {
        self.kind = None;
        self.open = false;
    }

    pub(super) fn show(&mut self) {
        self.kind = None;
        self.open = true;
    }

    pub(super) fn apply_regular(&self, text: String, context: &mut AppContext) {
        let mut ignored = Bus::new();
        RefreshRateRegularSetting.handle(
            &Event::Submit(ViewId::RefreshRateRegularInput, text),
            context,
            &mut ignored,
        );
    }

    pub(super) fn apply_inverted(&self, text: String, context: &mut AppContext) {
        let mut ignored = Bus::new();
        RefreshRateInvertedSetting.handle(
            &Event::Submit(ViewId::RefreshRateInvertedInput, text),
            context,
            &mut ignored,
        );
    }

    pub(super) fn open_kind(&mut self, ext: FileExtension, context: &AppContext) {
        let pair = context
            .settings
            .reader
            .refresh_rate
            .by_kind
            .get(&ext)
            .cloned()
            .unwrap_or(RefreshRatePair {
                regular: 0,
                inverted: 0,
            });
        self.kind = Some(KindDraft { ext, pair });
        self.open = true;
    }

    pub(super) fn delete_kind(&mut self, ext: FileExtension, context: &mut AppContext) {
        context.settings.reader.refresh_rate.by_kind.remove(&ext);
        if self.kind.as_ref().is_some_and(|draft| draft.ext == ext) {
            self.kind = None;
        }
    }

    pub(super) fn apply_kind_text(&mut self, regular: bool, text: String) {
        let Ok(value) = text.parse::<u8>() else {
            return;
        };
        let Some(draft) = &mut self.kind else {
            return;
        };
        if regular {
            draft.pair.regular = value;
        } else {
            draft.pair.inverted = value;
        }
    }

    fn accept_kind(&mut self, context: &mut AppContext) {
        if let Some(draft) = self.kind.take() {
            context
                .settings
                .reader
                .refresh_rate
                .by_kind
                .insert(draft.ext, draft.pair);
        }
    }

    pub(super) fn bump(window: &SettingsWindow, delta: i32) {
        let io = window.global::<RefreshRatesIo>();
        io.set_page(io.get_page().saturating_add(delta).max(0));
    }

    fn kind_text(&self, regular: bool) -> Option<(String, String)> {
        let draft = self.kind.as_ref()?;
        let ext = draft.ext.as_str();
        if regular {
            Some((
                fl!(
                    "settings-reader-refresh-rate-by-kind-regular-input",
                    ext = ext
                ),
                draft.pair.regular.to_string(),
            ))
        } else {
            Some((
                fl!(
                    "settings-reader-refresh-rate-by-kind-inverted-input",
                    ext = ext
                ),
                draft.pair.inverted.to_string(),
            ))
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for RefreshRates {
    type Window = SettingsWindow;
    type Action = Action;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<RefreshRatesIo>();
        connect!(io, self.queue, on_regular_pressed, () => Action::Regular);
        connect!(io, self.queue, on_inverted_pressed, () => Action::Inverted);
        connect!(io, self.queue, on_kind_pressed, (index) => Action::KindPressed(index as usize));
        connect!(io, self.queue, on_kind_held, (index) => Action::KindHeld(index as usize));
        connect!(io, self.queue, on_add_pressed, () => Action::Add);
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
        connect!(io, self.queue, on_kind_regular_pressed, () => Action::KindRegularPressed);
        connect!(io, self.queue, on_kind_inverted_pressed, () => Action::KindInvertedPressed);
        connect!(io, self.queue, on_kind_accepted, () => Action::KindAccepted);
        connect!(io, self.queue, on_kind_dismissed, () => Action::KindDismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<RefreshRatesIo>();
        if !self.open {
            io.set_page(0);
        }
        io.set_open(self.open);
        io.set_regular(
            context
                .settings
                .reader
                .refresh_rate
                .global
                .regular
                .to_string()
                .into(),
        );
        io.set_inverted(
            context
                .settings
                .reader
                .refresh_rate
                .global
                .inverted
                .to_string()
                .into(),
        );
        if let Some(kinds) = changed_model(&io.get_kinds(), kind_lines(context)) {
            io.set_kinds(kinds);
        }
        let kind_open = self.kind.is_some();
        let kind_title = self
            .kind
            .as_ref()
            .map(|draft| draft.ext.to_string().to_uppercase())
            .unwrap_or_default();
        let kind_regular = self
            .kind
            .as_ref()
            .map(|draft| draft.pair.regular.to_string())
            .unwrap_or_default();
        let kind_inverted = self
            .kind
            .as_ref()
            .map(|draft| draft.pair.inverted.to_string())
            .unwrap_or_default();
        io.set_kind_open(kind_open);
        io.set_kind_title(kind_title.into());
        io.set_kind_regular(kind_regular.into());
        io.set_kind_inverted(kind_inverted.into());
    }

    async fn apply(&mut self, action: Action, env: &mut Env<'_>) -> Step {
        match action {
            Action::Dismissed => {
                self.close();
                Step::Redraw
            }
            Action::Regular => {
                let global = &env.context.settings.reader.refresh_rate.global;
                Step::text(
                    Waiting::RefreshRegular,
                    fl!("settings-reader-refresh-rate-regular-input"),
                    global.regular.to_string(),
                    text_limits::REFRESH_RATE,
                )
            }
            Action::Inverted => {
                let global = &env.context.settings.reader.refresh_rate.global;
                Step::text(
                    Waiting::RefreshInverted,
                    fl!("settings-reader-refresh-rate-inverted-input"),
                    global.inverted.to_string(),
                    text_limits::REFRESH_RATE,
                )
            }
            Action::KindPressed(index) => {
                let Some(ext) = kind_extensions(&env.context.settings).get(index).copied() else {
                    return Step::Idle;
                };
                self.open_kind(ext, env.context);
                Step::Redraw
            }
            Action::KindHeld(index) => {
                let Some(ext) = kind_extensions(&env.context.settings).get(index).copied() else {
                    return Step::Idle;
                };
                Step::choice(
                    Waiting::DeleteKind(ext),
                    ext.to_string(),
                    vec![EntryKind::Command(
                        fl!("delete"),
                        EntryId::DeleteRefreshRateByKind(ext),
                    )],
                )
            }
            Action::Add => {
                let existing: HashSet<FileExtension> = env
                    .context
                    .settings
                    .reader
                    .refresh_rate
                    .by_kind
                    .keys()
                    .copied()
                    .collect();
                let extensions: Vec<FileExtension> = FileExtension::all()
                    .iter()
                    .copied()
                    .filter(|ext| !existing.contains(ext))
                    .collect();
                if extensions.is_empty() {
                    return Step::Idle;
                }
                let entries = extensions
                    .iter()
                    .copied()
                    .map(|ext| {
                        EntryKind::Command(ext.to_string(), EntryId::EditRefreshRateByKind(ext))
                    })
                    .collect();
                Step::choice(
                    Waiting::AddKind,
                    fl!("settings-reader-refresh-rate"),
                    entries,
                )
            }
            Action::KindDismissed => {
                self.kind = None;
                Step::Redraw
            }
            Action::KindAccepted => {
                self.accept_kind(env.context);
                Step::Redraw
            }
            Action::KindRegularPressed => match self.kind_text(true) {
                Some((label, initial)) => Step::text(
                    Waiting::KindRegular,
                    label,
                    initial,
                    text_limits::REFRESH_RATE,
                ),
                None => Step::Idle,
            },
            Action::KindInvertedPressed => match self.kind_text(false) {
                Some((label, initial)) => Step::text(
                    Waiting::KindInverted,
                    label,
                    initial,
                    text_limits::REFRESH_RATE,
                ),
                None => Step::Idle,
            },
        }
    }
}

fn kind_extensions(settings: &crate::settings::Settings) -> Vec<FileExtension> {
    let mut extensions: Vec<FileExtension> = settings
        .reader
        .refresh_rate
        .by_kind
        .keys()
        .copied()
        .collect();
    extensions.sort_unstable();
    extensions
}

fn kind_lines(context: &AppContext) -> Vec<PagedLine> {
    kind_extensions(&context.settings)
        .into_iter()
        .map(|ext| {
            let pair = context.settings.reader.refresh_rate.by_kind.get(&ext);
            let regular = pair.map(|pair| pair.regular).unwrap_or(0);
            let inverted = pair.map(|pair| pair.inverted).unwrap_or(0);
            PagedLine {
                label: ext.to_string().to_uppercase().into(),
                value: format!("{regular} / {inverted}").into(),
                selected: false,
            }
        })
        .collect()
}
