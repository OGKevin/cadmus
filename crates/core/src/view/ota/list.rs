//! Pull-request list: rows, pager, refresh.

use crate::device::AppContext;
use crate::fl;
use crate::ota::{CheckedPull, PrBuild};
use crate::view::slint::generated::OtaWindow;
use crate::view::slint::{
    CallbackQueue, Component, Env, Pager, changed_model, connect, paged_line,
};

#[derive(Debug)]
pub(super) enum Action {
    Line(i32),
    Refresh,
    Previous,
    Next,
}

pub(super) struct List {
    queue: CallbackQueue<Action>,
}

impl List {
    pub(super) fn new() -> Self {
        Self {
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn publish(
        &self,
        window: &OtaWindow,
        pager: Pager,
        rows: &[CheckedPull],
        loading: bool,
    ) {
        window.set_loading(loading);
        window.set_can_previous(pager.can_previous());
        window.set_can_next(pager.can_next(rows.len()));
        window.set_page_label(pager.label(rows.len()).into());
        let lines = pager
            .slice(rows)
            .iter()
            .map(|row| {
                paged_line(
                    row.title.as_str(),
                    row_description(row),
                    row.selectable(),
                    row.build == PrBuild::Pending,
                )
            })
            .collect();
        if let Some(model) = changed_model(&window.get_lines(), lines) {
            window.set_lines(model);
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Component for List {
    type Window = OtaWindow;
    type Action = Action;
    type Effect = Action;

    fn wire(&self, window: &OtaWindow) {
        connect!(window, self.queue, on_line_pressed, (index) => Action::Line(index));
        connect!(window, self.queue, on_refresh_clicked, () => Action::Refresh);
        connect!(window, self.queue, on_previous_clicked, () => Action::Previous);
        connect!(window, self.queue, on_next_clicked, () => Action::Next);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    fn sync(&mut self, _window: &OtaWindow, _context: &AppContext) {}

    async fn apply(&mut self, action: Action, _env: &mut Env<'_>) -> Action {
        action
    }
}

pub(super) fn row_description(row: &CheckedPull) -> String {
    let status = match row.build {
        PrBuild::Pending => {
            return if row.draft {
                fl!(
                    "ota-pr-checking-draft",
                    number = row.number,
                    draft = fl!("ota-status-draft")
                )
            } else {
                fl!("ota-pr-checking", number = row.number)
            };
        }
        PrBuild::Ready => fl!("ota-status-ready"),
        PrBuild::Building => fl!("ota-status-building"),
        PrBuild::Failed => fl!("ota-status-failed"),
        PrBuild::NoBuild => fl!("ota-status-no-build"),
        PrBuild::Unknown => fl!("ota-status-unknown"),
    };
    if row.draft {
        fl!(
            "ota-pr-row-draft",
            number = row.number,
            draft = fl!("ota-status-draft"),
            status = status.as_str()
        )
    } else {
        fl!("ota-pr-row", number = row.number, status = status.as_str())
    }
}
