//! Shared option list behind the `ChoiceSheet` component.
//!
//! The component (`ui/widgets/choice_sheet.slint`) lists options under a title
//! and reports which row was tapped. `T` is the value of one row. The second
//! type parameter is the window that draws it: any window whose component
//! exports `ChoiceSheetIo`. A screen constructs `ChoiceSheet<ItsRow, ItsWindow>`
//! and remembers why it opened the list.

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::view::slint::component::{Component, Env};
use crate::view::slint::generated::{ChoiceEntry, ChoiceSheetIo};
use crate::view::slint::{CallbackQueue, changed_model, connect};

/// One row of an option list, carrying the screen's value for that row.
pub(in crate::view) struct Row<T> {
    pub(in crate::view) label: String,
    pub(in crate::view) checked: bool,
    pub(in crate::view) value: T,
}

/// One callback from the option list.
#[derive(Debug)]
pub(in crate::view) enum Action {
    /// The entry at this index, starting at 0, was tapped.
    Picked(usize),
    /// A tap outside the dialog closed it.
    Dismissed,
}

/// What the screen does after one callback.
pub(in crate::view) enum Outcome<T> {
    /// Nothing further.
    Idle,
    /// The reader closed the list.
    Dismissed,
    /// The reader picked this value. The list stays open until the screen
    /// closes it, so a row the screen ignores leaves the list on screen.
    Picked(T),
}

/// State of one open option list.
pub(in crate::view) struct ChoiceSheet<T, W> {
    open: bool,
    title: String,
    rows: Vec<Row<T>>,
    /// The visible page goes back to the first on the next sync.
    reset_page: bool,
    queue: CallbackQueue<Action>,
    window: super::WindowOf<W>,
}

impl<T, W> ChoiceSheet<T, W> {
    pub(in crate::view) fn new() -> Self {
        Self {
            open: false,
            title: String::new(),
            rows: Vec::new(),
            reset_page: false,
            queue: CallbackQueue::new(),
            window: super::WindowOf::default(),
        }
    }

    pub(in crate::view) fn is_open(&self) -> bool {
        self.open
    }

    /// Opens a new list on `title`, starting at its first page.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "choice_sheet.open",
            skip(self),
            fields(rows = rows.len(), title = %title)
        )
    )]
    pub(in crate::view) fn open(&mut self, title: String, rows: Vec<Row<T>>) {
        tracing::debug!("option list opened");
        self.open = true;
        self.title = title;
        self.rows = rows;
        self.reset_page = true;
    }

    /// Replaces the rows of the open list and keeps the reader's page.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "choice_sheet.set_rows", skip_all, fields(rows = rows.len()))
    )]
    pub(in crate::view) fn set_rows(&mut self, rows: Vec<Row<T>>) {
        self.rows = rows;
    }

    /// Closes the list. The next sync clears the dialog.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "choice_sheet.close", skip_all)
    )]
    pub(in crate::view) fn close(&mut self) {
        if !self.open {
            return;
        }
        tracing::debug!("option list closed");
        self.open = false;
        self.title.clear();
        self.rows.clear();
        self.reset_page = true;
    }

    /// Reports the row at `index` without closing.
    fn pick(&self, index: usize) -> Outcome<T>
    where
        T: Clone,
    {
        match self.rows.get(index) {
            Some(row) => Outcome::Picked(row.value.clone()),
            None => {
                tracing::debug!("option list ignored a row");
                Outcome::Idle
            }
        }
    }

    fn handle(&mut self, action: Action) -> Outcome<T>
    where
        T: Clone,
    {
        match action {
            Action::Picked(index) => self.pick(index),
            Action::Dismissed => {
                self.close();
                Outcome::Dismissed
            }
        }
    }
}

impl<T, W> ChoiceSheet<T, W>
where
    W: ComponentHandle,
    for<'a> ChoiceSheetIo<'a>: Global<'a, W>,
{
    /// Moves the visible page by `delta`. The sheet clamps it.
    pub(in crate::view) fn bump(window: &W, delta: i32) {
        let io = window.global::<ChoiceSheetIo>();
        io.set_page(io.get_page().saturating_add(delta).max(0));
    }
}

#[async_trait::async_trait(?Send)]
impl<T, W> Component for ChoiceSheet<T, W>
where
    T: Clone + 'static,
    W: ComponentHandle + 'static,
    for<'a> ChoiceSheetIo<'a>: Global<'a, W>,
{
    type Window = W;
    type Action = Action;
    type Effect = Outcome<T>;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "choice_sheet.wire", skip_all)
    )]
    fn wire(&self, window: &W) {
        let io = window.global::<ChoiceSheetIo>();
        connect!(io, self.queue, on_entry_pressed, (index) => Action::Picked(index as usize));
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "choice_sheet.sync",
            level = "trace",
            skip_all,
            fields(open = self.open, rows = self.rows.len())
        )
    )]
    fn sync(&mut self, window: &W, _context: &AppContext) {
        let io = window.global::<ChoiceSheetIo>();
        io.set_open(self.open);
        io.set_title(self.title.as_str().into());
        if std::mem::take(&mut self.reset_page) {
            io.set_page(0);
        }
        let entries = self
            .rows
            .iter()
            .map(|row| ChoiceEntry {
                label: row.label.as_str().into(),
                checked: row.checked,
            })
            .collect();
        if let Some(model) = changed_model(&io.get_entries(), entries) {
            io.set_entries(model);
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "choice_sheet.apply", skip_all, fields(action = ?action))
    )]
    async fn apply(&mut self, action: Action, _env: &mut Env<'_>) -> Outcome<T> {
        self.handle(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> ChoiceSheet<&'static str, ()> {
        ChoiceSheet::new()
    }

    fn one_row() -> Vec<Row<&'static str>> {
        vec![Row {
            label: "English".into(),
            checked: true,
            value: "en",
        }]
    }

    #[test]
    fn picking_a_row_leaves_the_list_open() {
        let mut sheet = sheet();
        sheet.open("Language".into(), one_row());
        assert!(matches!(
            sheet.handle(Action::Picked(0)),
            Outcome::Picked("en")
        ));
        assert!(sheet.is_open(), "picking a row closed the list");
        assert!(matches!(sheet.handle(Action::Picked(3)), Outcome::Idle));
        assert!(sheet.is_open(), "an unknown row closed the list");
    }

    #[test]
    fn dismissing_closes_the_list() {
        let mut sheet = sheet();
        sheet.open("Language".into(), one_row());
        assert!(matches!(
            sheet.handle(Action::Dismissed),
            Outcome::Dismissed
        ));
        assert!(!sheet.is_open());
    }
}
