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
        io.set_heading(fl!("settings-reader-refresh-rate").into());
        io.set_regular_label(fl!("settings-reader-refresh-rate-regular").into());
        io.set_inverted_label(fl!("settings-reader-refresh-rate-inverted").into());
        io.set_accept_label(fl!("ok").into());
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

#[cfg(test)]
mod tests {
    use slint::ComponentHandle;
    use slint::Model;

    use super::super::settings_env;
    use super::super::text_limits;
    use super::super::{Sheet, Waiting};
    use super::{Action, Component, RefreshRates, Step};
    use crate::document::file_extension::FileExtension;
    use crate::fl;
    use crate::settings::RefreshRatePair;
    use crate::view::slint::generated::{PagedLine, RefreshRatesIo};
    use crate::view::{EntryId, EntryKind};

    fn kind_at(kinds: &slint::ModelRc<PagedLine>, index: usize) -> PagedLine {
        kinds
            .row_data(index)
            .unwrap_or_else(|| panic!("missing kind {index}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sync_shows_the_rates_and_resets_the_page_when_closed() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        context.settings.reader.refresh_rate.global.regular = 12;
        context.settings.reader.refresh_rate.global.inverted = 3;
        context.settings.reader.refresh_rate.by_kind.insert(
            FileExtension::Epub,
            RefreshRatePair {
                regular: 4,
                inverted: 5,
            },
        );

        let mut rates = RefreshRates::new();
        rates.show();
        rates.sync(window, &context);
        let io = window.global::<RefreshRatesIo>();
        assert!(io.get_open());
        assert!(!io.get_kind_open());
        assert_eq!(
            io.get_heading().as_str(),
            fl!("settings-reader-refresh-rate")
        );
        assert_eq!(io.get_regular().as_str(), "12");
        assert_eq!(io.get_inverted().as_str(), "3");
        assert_eq!(io.get_kinds().row_count(), 1);
        let kind = kind_at(&io.get_kinds(), 0);
        assert_eq!(kind.label.as_str(), "EPUB");
        assert_eq!(kind.value.as_str(), "4 / 5");
        assert!(!kind.selected);

        RefreshRates::bump(window, 2);
        rates.sync(window, &context);
        assert_eq!(window.global::<RefreshRatesIo>().get_page(), 2);

        rates.close();
        rates.sync(window, &context);
        let io = window.global::<RefreshRatesIo>();
        assert!(!io.get_open());
        assert_eq!(io.get_page(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn callbacks_edit_the_global_rate_and_one_kind() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut rates = RefreshRates::new();
        rates.wire(window);
        rates.show();

        window.global::<RefreshRatesIo>().invoke_regular_pressed();
        window.global::<RefreshRatesIo>().invoke_inverted_pressed();
        window.global::<RefreshRatesIo>().invoke_add_pressed();
        window.global::<RefreshRatesIo>().invoke_kind_pressed(0);
        assert!(matches!(
            rates.take().as_slice(),
            [
                Action::Regular,
                Action::Inverted,
                Action::Add,
                Action::KindPressed(0)
            ]
        ));

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let regular = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::Regular, &mut env).await
        };
        let Step::Ask(
            Waiting::RefreshRegular,
            Sheet::Text {
                initial, limits, ..
            },
        ) = regular
        else {
            panic!("regular press did not open the rate field");
        };
        assert_eq!(
            initial,
            context
                .settings
                .reader
                .refresh_rate
                .global
                .regular
                .to_string()
        );
        assert_eq!(limits, text_limits::REFRESH_RATE);

        rates.apply_regular("15".into(), &mut context);
        rates.apply_regular("nope".into(), &mut context);
        rates.sync(window, &context);
        assert_eq!(
            window.global::<RefreshRatesIo>().get_regular().as_str(),
            "15"
        );
        assert_eq!(context.settings.reader.refresh_rate.global.regular, 15);

        let inverted = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::Inverted, &mut env).await
        };
        let Step::Ask(Waiting::RefreshInverted, Sheet::Text { initial, .. }) = inverted else {
            panic!("inverted press did not open the rate field");
        };
        assert_eq!(
            initial,
            context
                .settings
                .reader
                .refresh_rate
                .global
                .inverted
                .to_string()
        );
        rates.apply_inverted("4".into(), &mut context);
        assert_eq!(context.settings.reader.refresh_rate.global.inverted, 4);

        let add = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::Add, &mut env).await
        };
        let Step::Ask(Waiting::AddKind, Sheet::Choice { entries, .. }) = add else {
            panic!("add did not open the format list");
        };
        assert_eq!(entries.len(), FileExtension::all().len());
        assert!(entries.iter().any(|entry| matches!(
            entry,
            EntryKind::Command(_, EntryId::EditRefreshRateByKind(FileExtension::Epub))
        )));

        let missing = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindPressed(0), &mut env).await
        };
        assert!(matches!(missing, Step::Idle));
        assert!(!rates.kind_is_open());

        rates.open_kind(FileExtension::Epub, &context);
        rates.apply_kind_text(true, "9".into());
        rates.apply_kind_text(true, "256".into());
        rates.apply_kind_text(false, "1".into());
        rates.sync(window, &context);
        let io = window.global::<RefreshRatesIo>();
        assert!(io.get_kind_open());
        assert_eq!(io.get_kind_title().as_str(), "EPUB");
        assert_eq!(io.get_kind_regular().as_str(), "9");
        assert_eq!(io.get_kind_inverted().as_str(), "1");

        let kind_regular = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindRegularPressed, &mut env).await
        };
        let Step::Ask(
            Waiting::KindRegular,
            Sheet::Text {
                initial, limits, ..
            },
        ) = kind_regular
        else {
            panic!("kind regular press did not open the rate field");
        };
        assert_eq!(initial, "9");
        assert_eq!(limits, text_limits::REFRESH_RATE);

        let kind_inverted = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindInvertedPressed, &mut env).await
        };
        let Step::Ask(
            Waiting::KindInverted,
            Sheet::Text {
                initial, limits, ..
            },
        ) = kind_inverted
        else {
            panic!("kind inverted press did not open the rate field");
        };
        assert_eq!(initial, "1");
        assert_eq!(limits, text_limits::REFRESH_RATE);

        let accepted = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindAccepted, &mut env).await
        };
        assert!(matches!(accepted, Step::Redraw));
        assert!(!rates.kind_is_open());
        assert_eq!(
            context
                .settings
                .reader
                .refresh_rate
                .by_kind
                .get(&FileExtension::Epub)
                .map(|pair| (pair.regular, pair.inverted)),
            Some((9, 1))
        );
        rates.sync(window, &context);
        let kind = kind_at(&window.global::<RefreshRatesIo>().get_kinds(), 0);
        assert_eq!(kind.label.as_str(), "EPUB");
        assert_eq!(kind.value.as_str(), "9 / 1");

        let again = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::Add, &mut env).await
        };
        let Step::Ask(Waiting::AddKind, Sheet::Choice { entries, .. }) = again else {
            panic!("add did not open the remaining formats");
        };
        assert_eq!(entries.len(), FileExtension::all().len() - 1);
        assert!(entries.iter().all(|entry| !matches!(
            entry,
            EntryKind::Command(_, EntryId::EditRefreshRateByKind(FileExtension::Epub))
        )));

        let held = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindHeld(0), &mut env).await
        };
        let Step::Ask(Waiting::DeleteKind(ext), Sheet::Choice { entries, .. }) = held else {
            panic!("holding a kind did not offer delete");
        };
        assert_eq!(ext, FileExtension::Epub);
        assert!(matches!(
            entries.as_slice(),
            [EntryKind::Command(
                _,
                EntryId::DeleteRefreshRateByKind(FileExtension::Epub)
            )]
        ));

        rates.delete_kind(FileExtension::Epub, &mut context);
        rates.sync(window, &context);
        assert_eq!(window.global::<RefreshRatesIo>().get_kinds().row_count(), 0);

        rates.open_kind(FileExtension::Pdf, &context);
        let dismissed_kind = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::KindDismissed, &mut env).await
        };
        assert!(matches!(dismissed_kind, Step::Redraw));
        assert!(!rates.kind_is_open());
        assert!(rates.is_open());

        window.global::<RefreshRatesIo>().invoke_dismissed();
        assert!(matches!(rates.take().as_slice(), [Action::Dismissed]));
        let dismissed = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            rates.apply(Action::Dismissed, &mut env).await
        };
        assert!(matches!(dismissed, Step::Redraw));
        assert!(!rates.is_open());
        rates.sync(window, &context);
        assert!(!window.global::<RefreshRatesIo>().get_open());
    }
}
