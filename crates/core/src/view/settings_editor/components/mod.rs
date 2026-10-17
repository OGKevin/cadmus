//! Rust state and callback handling for settings UI components.
//!
//! Settings-only pieces implement [`Component`] and return [`Step`]. Shared
//! overlays come from [`crate::view::slint::components`], parameterized only
//! by this window (and, for the option list, by the row value). Each overlay's
//! result converts into [`Step`] with `From`. [`Step`] reduces to [`Flow`],
//! which tells the view loop whether the renderer should redraw.
//!
//! [`each_part`] is the list of pieces. Wire and drain walk it, so a new piece
//! is added in one place. Sync does not: it covers the shell,
//! the visible category page, and the overlays, because a hidden page is
//! repainted when its category opens.
//!
//! The Slint/Rust composition follows the global-callback pattern described in
//! [`crate::view::slint::renderer`] and [slint-ui/slint#2031].
//!
//! [slint-ui/slint#2031]: https://github.com/slint-ui/slint/issues/2031

mod category_bar;
mod dictionaries;
mod footer;
mod general;
mod import;
mod intermission;
mod libraries;
mod library_editor;
mod power;
mod reader;
mod refresh_rates;
mod setting;
mod shell;
mod telemetry;
mod text_limits;

use std::path::PathBuf;

use dictionaries::Dictionaries;
use library_editor::LibraryEditor;
use refresh_rates::RefreshRates;
use shell::Shell;
use text_limits::TextLimits;

use crate::device::AppContext;
use crate::document::file_extension::FileExtension;
use crate::view::slint::Flow;
use crate::view::slint::Session;
use crate::view::slint::components::{choice_sheet, confirm_sheet, file_browser, text_sheet};
use crate::view::slint::generated::{SettingRow, SettingsCategory, SettingsWindow};
pub(super) use crate::view::slint::{Component, Env};
use crate::view::{Bus, EntryId, EntryKind, Event, Hub};

pub(super) use file_browser::Mode as FilesMode;
#[cfg(test)]
pub(in crate::view::settings_editor) use shell::install_ui_font;
pub(super) use shell::open;

/// Walks every settings piece. The caller supplies what to do with one field.
macro_rules! each_part {
    ($self:ident, $call:ident) => {
        $call!($self, shell);
        $call!($self, category_bar);
        $call!($self, footer);
        $call!($self, general);
        $call!($self, power);
        $call!($self, reader);
        $call!($self, intermission);
        $call!($self, import);
        $call!($self, telemetry);
        $call!($self, libraries);
        $call!($self, dictionaries);
        $call!($self, files);
        $call!($self, choice);
        $call!($self, text);
        $call!($self, confirm);
        $call!($self, refresh);
        $call!($self, library);
    };
}

#[cfg(test)]
pub(super) use setting::shown_kinds;

#[cfg(test)]
pub(in crate::view::settings_editor) use setting::row_value;

/// Borrows the three fields of [`Env`] in field order.
#[cfg(test)]
pub(super) fn settings_env<'a>(
    hub: &'a Hub,
    bus: &'a mut Bus,
    context: &'a mut AppContext,
) -> Env<'a> {
    Env { hub, bus, context }
}

/// The text of the next notification on `rx`.
#[cfg(test)]
pub(super) fn notification(rx: &mut crate::view::HubReceiver) -> String {
    use crate::view::NotificationEvent;

    let message = rx.try_recv().expect("missing notification");
    match message.event {
        Event::Notification(NotificationEvent::Show(text)) => text,
        other => panic!("unexpected hub event {other:?}"),
    }
}

use setting::PressedRow;

/// What one settings callback asks the coordinator to do.
///
/// This is not a renderer instruction. [`Parts::dispatch`] reduces it to [`Flow`].
enum Step {
    /// Nothing further.
    Idle,
    /// The frame changed.
    Redraw,
    /// Leave the settings screen.
    Leave,
    /// Switch to another category.
    Category(SettingsCategory),
    /// Open a shared sheet, remembering which control asked.
    Ask(Waiting, Sheet),
    /// Open one of the settings editors.
    Edit(Editor),
    /// Replace the rows of the open option list. The list stays open.
    Rows(Vec<EntryKind>),
    /// The reader dismissed the open sheet.
    Dismissed,
    /// The choice sheet picked `id`.
    Picked(EntryId),
    /// The file browser chose `path`.
    ChoseFile(PathBuf),
    /// The text sheet submitted `text`.
    Submitted(String),
    /// The confirm sheet accepted.
    Confirmed,
}

impl Step {
    fn choice(waiting: Waiting, title: String, entries: Vec<EntryKind>) -> Self {
        Self::Ask(waiting, Sheet::Choice { title, entries })
    }

    fn text(waiting: Waiting, label: String, initial: String, limits: TextLimits) -> Self {
        Self::Ask(
            waiting,
            Sheet::Text {
                label,
                initial,
                limits,
            },
        )
    }

    fn file(waiting: Waiting, start: PathBuf, mode: FilesMode) -> Self {
        Self::Ask(waiting, Sheet::File { start, mode })
    }

    fn confirm(waiting: Waiting, message: String, accept: String) -> Self {
        Self::Ask(waiting, Sheet::Confirm { message, accept })
    }

    /// A name for spans. Typed text never appears in one.
    #[cfg(feature = "tracing")]
    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Redraw => "redraw",
            Self::Leave => "leave",
            Self::Category(_) => "category",
            Self::Ask(_, Sheet::Choice { .. }) => "ask_choice",
            Self::Ask(_, Sheet::Text { .. }) => "ask_text",
            Self::Ask(_, Sheet::File { .. }) => "ask_file",
            Self::Ask(_, Sheet::Confirm { .. }) => "ask_confirm",
            Self::Edit(_) => "edit",
            Self::Rows(_) => "rows",
            Self::Dismissed => "dismissed",
            Self::Picked(_) => "picked",
            Self::ChoseFile(_) => "file",
            Self::Submitted(_) => "text",
            Self::Confirmed => "confirmed",
        }
    }
}

/// A shared sheet to open. It does not store which control asked.
enum Sheet {
    Choice {
        title: String,
        entries: Vec<EntryKind>,
    },
    Text {
        label: String,
        initial: String,
        limits: TextLimits,
    },
    File {
        start: PathBuf,
        mode: FilesMode,
    },
    Confirm {
        message: String,
        accept: String,
    },
}

/// A settings editor that fills the screen.
#[derive(Debug)]
enum Editor {
    RefreshRates,
    /// An existing library by index, or a new library when `None`.
    Library(Option<usize>),
}

/// Which control is waiting on the open shared sheet.
///
/// One option list, file browser, text dialog, and confirmation dialog serve
/// every settings control. The overlay returns the pick. This remembers who
/// asked, because that is settings state, not part of the overlay.
#[derive(Debug)]
enum Waiting {
    SettingChoice(PressedRow),
    SettingText(PressedRow),
    SettingFile(SettingRow),
    ForceImport,
    LibraryName,
    LibraryPath,
    LibraryFinished,
    RefreshRegular,
    RefreshInverted,
    KindRegular,
    KindInverted,
    AddKind,
    DeleteKind(FileExtension),
    DeleteLibrary(usize),
    Dictionary,
    Download(String),
}

impl From<choice_sheet::Outcome<EntryId>> for Step {
    fn from(outcome: choice_sheet::Outcome<EntryId>) -> Self {
        match outcome {
            choice_sheet::Outcome::Idle => Self::Idle,
            choice_sheet::Outcome::Dismissed => Self::Dismissed,
            choice_sheet::Outcome::Picked(id) => Self::Picked(id),
        }
    }
}

impl From<file_browser::Outcome> for Step {
    fn from(outcome: file_browser::Outcome) -> Self {
        match outcome {
            file_browser::Outcome::Idle => Self::Idle,
            file_browser::Outcome::Changed => Self::Redraw,
            file_browser::Outcome::Dismissed => Self::Dismissed,
            file_browser::Outcome::Chosen(path) => Self::ChoseFile(path),
        }
    }
}

impl From<text_sheet::Outcome> for Step {
    fn from(outcome: text_sheet::Outcome) -> Self {
        match outcome {
            text_sheet::Outcome::Idle => Self::Idle,
            text_sheet::Outcome::Changed => Self::Redraw,
            text_sheet::Outcome::Dismissed => Self::Dismissed,
            text_sheet::Outcome::Accepted(text) => Self::Submitted(text),
        }
    }
}

impl From<confirm_sheet::Outcome> for Step {
    fn from(outcome: confirm_sheet::Outcome) -> Self {
        match outcome {
            confirm_sheet::Outcome::Idle => Self::Idle,
            confirm_sheet::Outcome::Dismissed => Self::Dismissed,
            confirm_sheet::Outcome::Accepted => Self::Confirmed,
        }
    }
}

/// Rows for the option list. An entry with no id cannot be picked, so it is
/// not listed.
fn choice_rows(entries: Vec<EntryKind>) -> Vec<choice_sheet::Row<EntryId>> {
    entries
        .into_iter()
        .filter_map(|entry| {
            Some(choice_sheet::Row {
                value: setting::entry_id(&entry)?,
                label: entry.text().to_owned(),
                checked: entry.get().unwrap_or(false),
            })
        })
        .collect()
}

type ChoiceList = choice_sheet::ChoiceSheet<EntryId, SettingsWindow>;
type Files = file_browser::FileBrowser<SettingsWindow>;

/// Settings component state coordinated by the editor.
pub(super) struct Parts {
    shell: Shell,
    category_bar: category_bar::CategoryBar,
    footer: footer::Footer,
    general: general::GeneralPage,
    power: power::PowerPage,
    reader: reader::ReaderPage,
    intermission: intermission::IntermissionPage,
    import: import::ImportPage,
    telemetry: telemetry::TelemetryPage,
    libraries: libraries::Libraries,
    dictionaries: Dictionaries,
    files: Files,
    choice: ChoiceList,
    text: text_sheet::TextSheet<SettingsWindow>,
    confirm: confirm_sheet::ConfirmSheet<SettingsWindow>,
    refresh: RefreshRates,
    library: LibraryEditor,
    /// Which control is waiting on the open shared sheet.
    waiting: Option<Waiting>,
}

fn elapsed_us(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Parts {
    pub(super) fn new(service: Option<crate::dictionary::MonolingualDictionaryService>) -> Self {
        Self {
            shell: Shell::new(),
            category_bar: category_bar::CategoryBar::new(),
            footer: footer::Footer::new(),
            general: general::GeneralPage::new(),
            power: power::PowerPage::new(),
            reader: reader::ReaderPage::new(),
            intermission: intermission::IntermissionPage::new(),
            import: import::ImportPage::new(),
            telemetry: telemetry::TelemetryPage::new(),
            libraries: libraries::Libraries::new(),
            dictionaries: Dictionaries::new(service),
            files: Files::new(),
            choice: ChoiceList::new(),
            text: text_sheet::TextSheet::new(),
            confirm: confirm_sheet::ConfirmSheet::new(),
            refresh: RefreshRates::new(),
            library: LibraryEditor::new(),
            waiting: None,
        }
    }

    #[cfg(test)]
    pub(super) fn category(&self) -> SettingsCategory {
        self.shell.category()
    }

    #[cfg(test)]
    pub(super) fn take_category_bar(&self) -> Vec<SettingsCategory> {
        self.category_bar.take()
    }

    /// Connects every component's callbacks. Runs once after the window opens.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(super) fn wire(&self, session: &Session<SettingsWindow>) {
        let window = session.component();
        macro_rules! wire_one {
            ($self:ident, $field:ident) => {
                $self.$field.wire(window);
            };
        }
        each_part!(self, wire_one);
    }

    /// Pushes component state into the window.
    ///
    /// Covers the shell, the visible category page, and the overlays. A page
    /// that is not showing is skipped: two of them read the disk to build
    /// their rows, and the page is synced when its category opens.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(super) fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let started = std::time::Instant::now();
        macro_rules! sync_one {
            ($self:ident, $field:ident) => {
                tracing::trace_span!("settings_part_sync", component = stringify!($field))
                    .in_scope(|| $self.$field.sync(window, context))
            };
        }
        sync_one!(self, shell);
        sync_one!(self, category_bar);
        sync_one!(self, footer);
        match self.shell.category() {
            SettingsCategory::General => sync_one!(self, general),
            SettingsCategory::Power => sync_one!(self, power),
            SettingsCategory::Reader => sync_one!(self, reader),
            SettingsCategory::Intermissions => sync_one!(self, intermission),
            SettingsCategory::Import => sync_one!(self, import),
            SettingsCategory::Telemetry => sync_one!(self, telemetry),
            SettingsCategory::Libraries => sync_one!(self, libraries),
            SettingsCategory::Dictionaries => sync_one!(self, dictionaries),
        }
        sync_one!(self, files);
        sync_one!(self, choice);
        sync_one!(self, text);
        sync_one!(self, confirm);
        sync_one!(self, refresh);
        sync_one!(self, library);
        tracing::trace!(elapsed_us = elapsed_us(started), "settings sync");
    }

    /// Applies every callback the window queued since the last drain.
    ///
    /// Returns what the renderer should do. Each component is drained and its
    /// returned [`Step`] routed before the next component, so an overlay opened
    /// by one component is visible to the next.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip_all, ret(level = tracing::Level::TRACE))
    )]
    pub(super) async fn drain(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        context: &mut AppContext,
    ) -> Flow {
        let mut env = Env { hub, bus, context };
        let mut flow = Flow::Idle;
        macro_rules! step {
            ($self:ident, $field:ident) => {{
                for _action in $self.$field.take() {
                    let effect = $self.$field.apply(_action, &mut env).await;
                    flow = flow.join($self.dispatch(effect.into(), &mut env).await);
                }
            }};
        }
        each_part!(self, step);
        flow
    }

    /// Performs one [`Step`], re-dispatching any step a route produces.
    ///
    /// The value returned to the view loop is a [`Flow`]: idle, or redraw.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip_all, fields(step = step.name()))
    )]
    async fn dispatch(&mut self, step: Step, env: &mut Env<'_>) -> Flow {
        let mut step = step;
        loop {
            step = match step {
                Step::Idle => return Flow::Idle,
                Step::Redraw => return Flow::Redraw,
                Step::Leave => {
                    tracing::debug!("settings leaving");
                    env.bus.push_back(Event::Back);
                    return Flow::Redraw;
                }
                Step::Category(category) => {
                    self.set_category(category, env.context).await;
                    return Flow::Redraw;
                }
                Step::Dismissed => {
                    self.waiting = None;
                    return Flow::Redraw;
                }
                Step::Ask(waiting, sheet) => {
                    self.open_sheet(waiting, sheet).await;
                    return Flow::Redraw;
                }
                Step::Edit(editor) => {
                    self.open_editor(editor, env.context);
                    return Flow::Redraw;
                }
                Step::Rows(entries) => {
                    self.choice.set_rows(choice_rows(entries));
                    return Flow::Redraw;
                }
                Step::Picked(id) => {
                    let waiting = self.waiting.take();
                    tracing::debug!(?waiting, "settings choice");
                    let next = self.route_choice(waiting.as_ref(), id, env).await;
                    self.finish_pick(waiting, &next);
                    next
                }
                Step::ChoseFile(path) => {
                    let waiting = self.waiting.take();
                    tracing::debug!(?waiting, "settings file chosen");
                    self.route_file(waiting, path, env.context);
                    return Flow::Redraw;
                }
                Step::Submitted(text) => {
                    let waiting = self.waiting.take();
                    tracing::debug!(?waiting, "settings text submitted");
                    self.route_text(waiting, text, env);
                    return Flow::Redraw;
                }
                Step::Confirmed => {
                    let waiting = self.waiting.take();
                    tracing::debug!(?waiting, "settings confirmed");
                    self.route_confirm(waiting, env).await
                }
            };
        }
    }

    /// Opens a shared sheet for the control `waiting` names.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    async fn open_sheet(&mut self, waiting: Waiting, sheet: Sheet) {
        self.waiting = Some(waiting);
        match sheet {
            Sheet::Choice { title, entries } => {
                self.text.close();
                self.choice.open(title, choice_rows(entries));
            }
            Sheet::Text {
                label,
                initial,
                limits,
            } => {
                self.choice.close();
                self.text
                    .open(label, initial, limits.numbers, limits.max_chars);
            }
            Sheet::File { start, mode } => {
                self.choice.close();
                self.text.close();
                self.files.open(start, mode).await;
            }
            Sheet::Confirm { message, accept } => self.confirm.open(message, accept),
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context)))]
    fn open_editor(&mut self, editor: Editor, context: &AppContext) {
        self.waiting = None;
        self.choice.close();
        self.text.close();
        match editor {
            Editor::RefreshRates => self.refresh.show(),
            Editor::Library(index) => self.library.open(index, context),
        }
    }

    /// Closes the option list when `next` accepted the row.
    ///
    /// [`Step::Idle`] is a row the screen ignored, and [`Step::Rows`] refreshes
    /// the same list. Either way the list stays open and `waiting` still names
    /// the control that opened it.
    fn finish_pick(&mut self, waiting: Option<Waiting>, next: &Step) {
        if matches!(next, Step::Idle | Step::Rows(_)) {
            self.waiting = waiting;
        } else {
            self.choice.close();
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, env), fields(?id))
    )]
    async fn route_choice(
        &mut self,
        waiting: Option<&Waiting>,
        id: EntryId,
        env: &mut Env<'_>,
    ) -> Step {
        match waiting {
            Some(Waiting::SettingChoice(row)) => setting::apply_setting_choice(*row, id, env),
            Some(Waiting::LibraryFinished) => {
                self.library.apply_finished(&id);
                Step::Redraw
            }
            Some(Waiting::AddKind) => match id {
                EntryId::EditRefreshRateByKind(ext) => {
                    self.refresh.open_kind(ext, env.context);
                    Step::Redraw
                }
                _ => Step::Idle,
            },
            Some(Waiting::DeleteLibrary(index)) => {
                libraries::delete(*index, env.context);
                Step::Redraw
            }
            Some(Waiting::DeleteKind(ext)) => {
                self.refresh.delete_kind(*ext, env.context);
                Step::Redraw
            }
            Some(Waiting::Dictionary) => self.dictionaries.apply_choice(&id, env).await,
            _ => Step::Idle,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, context), fields(path = %path.display()))
    )]
    fn route_file(&mut self, waiting: Option<Waiting>, path: PathBuf, context: &mut AppContext) {
        match waiting {
            Some(Waiting::LibraryPath) => self.library.set_path(path),
            Some(Waiting::SettingFile(row)) => setting::apply_file_choice(row, path, context),
            _ => {}
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, env, text), fields(len = text.chars().count()))
    )]
    fn route_text(&mut self, waiting: Option<Waiting>, text: String, env: &mut Env<'_>) {
        match waiting {
            Some(Waiting::SettingText(row)) => setting::apply_text(row, &text, env),
            Some(Waiting::LibraryName) => self.library.set_name(text),
            Some(Waiting::RefreshRegular) => self.refresh.apply_regular(text, env.context),
            Some(Waiting::RefreshInverted) => self.refresh.apply_inverted(text, env.context),
            Some(Waiting::KindRegular) => self.refresh.apply_kind_text(true, text),
            Some(Waiting::KindInverted) => self.refresh.apply_kind_text(false, text),
            _ => {}
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    async fn route_confirm(&mut self, waiting: Option<Waiting>, env: &mut Env<'_>) -> Step {
        match waiting {
            Some(Waiting::ForceImport) => {
                env.bus.push_back(Event::ImportLibrary {
                    library_index: None,
                    force: true,
                });
                Step::Redraw
            }
            Some(Waiting::Download(lang)) => self.dictionaries.start_download(&lang, env).await,
            _ => Step::Idle,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context), fields(?category)))]
    pub(super) async fn set_category(
        &mut self,
        category: SettingsCategory,
        context: &mut AppContext,
    ) {
        if category == self.shell.category() {
            return;
        }
        tracing::debug!(?category, "settings category");
        self.dismiss_all();
        self.shell.set_category(category);
        if category == SettingsCategory::Dictionaries {
            self.reload_dictionaries(context).await;
        }
    }

    /// Advances the category by `delta` with wrap-around, for the hardware keys.
    pub(super) async fn step_category(&mut self, delta: isize, context: &mut AppContext) {
        let categories = SettingsCategory::all();
        let current = categories
            .iter()
            .position(|category| *category == self.shell.category())
            .unwrap_or(0);
        let count = categories.len() as isize;
        let next = (current as isize + delta).rem_euclid(count) as usize;
        self.set_category(categories[next], context).await;
    }

    pub(super) fn is_plain(&self) -> bool {
        !self.choice.is_open()
            && !self.text.is_open()
            && !self.confirm.is_open()
            && !self.files.is_open()
            && !self.refresh.kind_is_open()
            && !self.refresh.is_open()
            && !self.library.is_open()
    }

    pub(super) fn close_top(&mut self) -> bool {
        let closed = if self.choice.is_open() || self.text.is_open() {
            self.choice.close();
            self.text.close();
            true
        } else if self.confirm.is_open() {
            self.confirm.close();
            true
        } else if self.files.is_open() {
            self.files.close();
            true
        } else if self.refresh.kind_is_open() {
            self.refresh.close_kind();
            true
        } else if self.refresh.is_open() {
            self.refresh.close();
            true
        } else if self.library.is_open() {
            self.library.close();
            true
        } else {
            false
        };
        if closed {
            self.waiting = None;
        }
        closed
    }

    pub(super) fn text_is_open(&self) -> bool {
        self.text.is_open()
    }

    /// Which open sheet should take the hardware page buttons.
    ///
    /// `Some` swallows forward and back so the settings page underneath stays
    /// still. The text dialog is not itself a page: claiming it only swallows
    /// those buttons. The choice list, file browser, and refresh-rate editor
    /// also move. A confirmation and the refresh-rate kind list are left
    /// unclaimed.
    pub(super) fn page_target(&self) -> Option<PageList> {
        if self.text.is_open() {
            return Some(PageList::Text);
        }
        if self.confirm.is_open() {
            return None;
        }
        if self.choice.is_open() {
            return Some(PageList::Choice);
        }
        if self.files.is_open() {
            return Some(PageList::Files);
        }
        if self.refresh.kind_is_open() {
            return None;
        }
        if self.refresh.is_open() {
            return Some(PageList::Refresh);
        }
        None
    }

    pub(super) fn bump(&self, window: &SettingsWindow, target: &PageList, delta: i32) {
        match target {
            PageList::Files => Files::bump(window, delta),
            PageList::Refresh => RefreshRates::bump(window, delta),
            PageList::Choice => ChoiceList::bump(window, delta),
            PageList::Text => {}
        }
    }

    pub(super) fn dismiss_all(&mut self) {
        self.waiting = None;
        self.choice.close();
        self.text.close();
        self.confirm.close();
        self.library.close();
        self.refresh.close();
        self.files.close();
    }

    pub(super) async fn reload_dictionaries(&mut self, context: &AppContext) {
        self.dictionaries.reload(context).await;
    }

    pub(super) fn stop_jobs(&self) {
        self.dictionaries.stop_jobs();
    }

    pub(super) fn take_job(&mut self) -> Option<crate::runtime::Job> {
        self.dictionaries.take_job()
    }

    #[cfg(test)]
    pub(super) fn dictionary_job(&self) -> Option<&crate::runtime::Job> {
        self.dictionaries.job()
    }

    #[cfg(test)]
    pub(super) fn set_dictionary_job(&mut self, job: Option<crate::runtime::Job>) {
        self.dictionaries.set_job(job);
    }

    #[cfg(test)]
    pub(super) fn choice_is_open(&self) -> bool {
        self.choice.is_open()
    }

    #[cfg(test)]
    pub(super) fn show_confirm(&mut self) {
        self.confirm
            .open("Delete this library?".into(), "Delete".into());
    }

    #[cfg(test)]
    pub(super) fn confirm_is_open(&self) -> bool {
        self.confirm.is_open()
    }

    #[cfg(test)]
    pub(super) async fn hold_library(&mut self, row: i32, context: &AppContext) {
        if let Step::Ask(waiting, sheet) = self.libraries.hold(row, context) {
            self.open_sheet(waiting, sheet).await;
        }
    }
}

/// Which open list the page buttons should move.
pub(super) enum PageList {
    Files,
    Refresh,
    Choice,
    Text,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::test_helpers::create_test_context;
    use crate::settings::LibrarySettings;
    use crate::view::slint::components::choice_sheet::Row;
    use crate::view::slint::generated::SettingRow;

    /// What `dispatch` does with an accepted row: it takes `waiting` before
    /// `finish_pick`. The list closes and that control is not put back.
    fn finish_accepted(parts: &mut Parts) {
        let waiting = parts.waiting.take();
        parts.finish_pick(waiting, &Step::Redraw);
    }

    fn row(label: &str, value: EntryId) -> Row<EntryId> {
        Row {
            label: label.to_owned(),
            checked: false,
            value,
        }
    }

    #[test]
    fn finish_pick_keeps_an_ignored_row_open() {
        let mut parts = Parts::new(None);
        parts.choice.open(
            "Format".into(),
            vec![row("leave", EntryId::DeleteLibrary(0))],
        );
        parts.finish_pick(Some(Waiting::AddKind), &Step::Idle);
        assert!(parts.choice.is_open());
        assert!(matches!(parts.waiting, Some(Waiting::AddKind)));

        parts.waiting = None;
        parts.finish_pick(Some(Waiting::AddKind), &Step::Rows(Vec::new()));
        assert!(parts.choice.is_open());
        assert!(matches!(parts.waiting, Some(Waiting::AddKind)));

        finish_accepted(&mut parts);
        assert!(!parts.choice.is_open());
        assert!(parts.waiting.is_none());
    }

    #[test]
    fn close_top_closes_the_front_sheet_and_forgets_who_asked() {
        let mut parts = Parts::new(None);
        parts
            .confirm
            .open("Delete this library?".into(), "Delete".into());
        parts.waiting = Some(Waiting::DeleteLibrary(1));
        assert!(parts.close_top());
        assert!(!parts.confirm.is_open());
        assert!(parts.waiting.is_none());

        parts.choice.open(
            "Language".into(),
            vec![row("English", EntryId::DeleteLibrary(0))],
        );
        parts.waiting = Some(Waiting::SettingChoice(PressedRow::Setting(
            SettingRow::Locale,
        )));
        assert!(parts.close_top());
        assert!(!parts.choice.is_open());
        assert!(parts.waiting.is_none());
        assert!(!parts.close_top());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_leaves_and_forgets_a_dismiss() {
        let mut context = create_test_context().await;
        let mut parts = Parts::new(None);
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        let mut env = super::settings_env(&hub, &mut bus, &mut context);
        parts.waiting = Some(Waiting::ForceImport);
        assert_eq!(
            parts.dispatch(Step::Dismissed, &mut env).await,
            Flow::Redraw
        );
        assert!(parts.waiting.is_none());

        assert_eq!(parts.dispatch(Step::Leave, &mut env).await, Flow::Redraw);
        assert!(matches!(bus.pop_front(), Some(Event::Back)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_keeps_an_ignored_pick_and_closes_an_accepted_one() {
        let mut context = create_test_context().await;
        context.settings.libraries.push(LibrarySettings {
            name: "Second".into(),
            ..LibrarySettings::default()
        });
        let before = context.settings.libraries.len();
        let index = before - 1;
        let mut parts = Parts::new(None);
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        let mut env = super::settings_env(&hub, &mut bus, &mut context);

        parts
            .open_sheet(
                Waiting::AddKind,
                Sheet::Choice {
                    title: "Format".into(),
                    entries: vec![EntryKind::Command(
                        "leave".into(),
                        EntryId::DeleteLibrary(0),
                    )],
                },
            )
            .await;
        assert_eq!(
            parts
                .dispatch(Step::Picked(EntryId::DeleteLibrary(0)), &mut env)
                .await,
            Flow::Idle
        );
        assert!(parts.choice.is_open(), "an ignored row closed the list");
        assert!(matches!(parts.waiting, Some(Waiting::AddKind)));

        parts
            .open_sheet(
                Waiting::DeleteLibrary(index),
                Sheet::Choice {
                    title: "Delete".into(),
                    entries: vec![EntryKind::Command(
                        "Delete".into(),
                        EntryId::DeleteLibrary(index),
                    )],
                },
            )
            .await;
        assert_eq!(
            parts
                .dispatch(Step::Picked(EntryId::DeleteLibrary(index)), &mut env)
                .await,
            Flow::Redraw
        );
        assert!(!parts.choice.is_open());
        assert!(parts.waiting.is_none());
        assert_eq!(env.context.settings.libraries.len(), before - 1);
    }
}
