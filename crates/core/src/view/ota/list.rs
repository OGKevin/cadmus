//! Pull-request list: rows and the row callback.

use crate::device::AppContext;
use crate::fl;
use crate::ota::{CheckedPull, PrBuild};
use crate::view::slint::generated::OtaWindow;
use crate::view::slint::{
    CallbackQueue, Component, Env, ListRow, changed_model, connect, paged_line,
};

#[derive(Debug)]
pub(super) enum Action {
    Line(i32),
    Refresh,
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

    /// Publishes every open pull request. The list shows one page of them.
    pub(super) fn publish(&self, window: &OtaWindow, rows: &[CheckedPull], loading: bool) {
        window.set_loading(loading);
        let lines = rows
            .iter()
            .map(|row| {
                paged_line(ListRow {
                    label: row.title.clone(),
                    description: row_description(row),
                    disabled: !row.selectable(),
                    busy: row.build == PrBuild::Pending,
                    ..Default::default()
                })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::slint::PagedIo;
    use crate::view::slint::generated::OtaScreen;
    use slint::Model as _;

    fn row(number: u64) -> CheckedPull {
        CheckedPull {
            number,
            title: format!("Pull {number}"),
            draft: false,
            build: PrBuild::Ready,
            artifact: Some(crate::ota::PrArtifact {
                id: number,
                size_in_bytes: 10,
            }),
        }
    }

    fn window() -> crate::view::slint::Session<OtaWindow> {
        let font = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fonts/NotoSans-Regular.ttf"),
        )
        .expect("noto sans");
        crate::view::slint::open_with_font(300, 1264, 1680, &font, |_family| {
            OtaWindow::new().map_err(Into::into)
        })
        .expect("ota session")
    }

    /// A frame. The list is built on the first one, so the page count is
    /// known from then on.
    fn frame(session: &crate::view::slint::Session<OtaWindow>) {
        assert!(session.redraw());
    }

    #[test]
    fn every_row_is_published_and_the_page_stays_within_the_list() {
        let session = window();
        session.component().set_screen(OtaScreen::List);
        let list = List::new();
        frame(&session);

        let rows: Vec<_> = (0..3).map(row).collect();
        list.publish(session.component(), &rows, false);
        assert_eq!(session.component().get_lines().row_count(), 3);
        frame(&session);

        session.component().bump(1);
        frame(&session);
        frame(&session);
        assert_eq!(
            session.component().get_page(),
            0,
            "a list that fits on one page cannot move"
        );

        let rows: Vec<_> = (0..200).map(row).collect();
        list.publish(session.component(), &rows, false);
        assert_eq!(session.component().get_lines().row_count(), 200);
        frame(&session);

        session.component().bump(1);
        frame(&session);
        frame(&session);
        assert_eq!(session.component().get_page(), 1);
        session.component().bump(-1);
        frame(&session);
        frame(&session);
        assert_eq!(session.component().get_page(), 0);
    }
}
