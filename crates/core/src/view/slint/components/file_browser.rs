//! Shared file browser behind the `FileChooser` component.
//!
//! The component (`ui/widgets/file_chooser.slint`) draws the path, the rows,
//! and the up/select actions. This type lists a directory and reports the
//! path the reader picked. The type parameter is the window that draws it:
//! any window whose component exports `FileBrowserIo`. A screen constructs
//! `FileBrowser<ItsWindow>` and remembers why it opened the browser.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::view::slint::component::{Component, Env};
use crate::view::slint::generated::{FileBrowserIo, PagedLine};
use crate::view::slint::{CallbackQueue, changed_model, connect};

/// Whether the browser lists files too, or only directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    File,
    Directory,
}

/// One row in the directory listing.
struct Entry {
    path: PathBuf,
    /// The name, with a trailing slash on a directory.
    label: String,
    is_dir: bool,
}

/// One callback from the file browser.
#[derive(Debug)]
pub enum Action {
    /// The row at this index, starting at 0, was tapped.
    Press(usize),
    /// The parent directory was requested.
    Up,
    /// The current directory was chosen.
    SelectFolder,
    /// The browser was closed.
    Dismissed,
}

/// What the screen does after one callback.
pub enum Outcome {
    /// Nothing further.
    Idle,
    /// The listing changed.
    Changed,
    /// The reader picked `path`. The browser is closed.
    Chosen(PathBuf),
    /// The reader closed the browser.
    Dismissed,
}

/// State of one open file browser.
pub struct FileBrowser<W> {
    state: Option<State>,
    queue: CallbackQueue<Action>,
    window: super::WindowOf<W>,
}

struct State {
    path: PathBuf,
    entries: Vec<Entry>,
    mode: Mode,
}

impl<W> FileBrowser<W> {
    pub fn new() -> Self {
        Self {
            state: None,
            queue: CallbackQueue::new(),
            window: super::WindowOf::default(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.state.is_some()
    }

    /// Opens the browser at `start`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "file_browser.open",
            skip(self),
            fields(path = %start.display(), ?mode)
        )
    )]
    pub async fn open(&mut self, start: PathBuf, mode: Mode) {
        tracing::debug!("file browser opened");
        self.load(start, mode).await;
    }

    /// Closes the browser.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "file_browser.close", skip_all)
    )]
    pub fn close(&mut self) {
        if self.state.take().is_some() {
            tracing::debug!("file browser closed");
        }
    }

    async fn press(&mut self, index: usize) -> Outcome {
        let Some(state) = &self.state else {
            return Outcome::Idle;
        };
        let Some(entry) = state.entries.get(index) else {
            return Outcome::Idle;
        };
        let (path, mode) = (entry.path.clone(), state.mode);
        if entry.is_dir {
            self.load(path, mode).await;
            Outcome::Changed
        } else if mode == Mode::File {
            self.choose(path)
        } else {
            Outcome::Idle
        }
    }

    async fn up(&mut self) -> Outcome {
        let Some(state) = &self.state else {
            return Outcome::Idle;
        };
        let Some(parent) = state
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
        else {
            return Outcome::Idle;
        };
        let mode = state.mode;
        self.load(parent, mode).await;
        Outcome::Changed
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "file_browser.choose",
            skip(self),
            fields(path = %path.display())
        )
    )]
    fn choose(&mut self, path: PathBuf) -> Outcome {
        if self.state.take().is_none() {
            return Outcome::Idle;
        }
        tracing::debug!("file browser chose");
        Outcome::Chosen(path)
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "file_browser.load",
            skip(self),
            fields(path = %path.display())
        )
    )]
    async fn load(&mut self, path: PathBuf, mode: Mode) {
        let entries = list_files(&path, mode).await;
        tracing::debug!(entries = entries.len(), "file browser listed a directory");
        self.state = Some(State {
            path,
            entries,
            mode,
        });
    }
}

impl<W> FileBrowser<W>
where
    W: ComponentHandle,
    for<'a> FileBrowserIo<'a>: Global<'a, W>,
{
    /// Moves the visible page by `delta`. The list clamps it.
    pub fn bump(window: &W, delta: i32) {
        let io = window.global::<FileBrowserIo>();
        io.set_page(io.get_page().saturating_add(delta).max(0));
    }
}

#[async_trait::async_trait(?Send)]
impl<W> Component for FileBrowser<W>
where
    W: ComponentHandle + 'static,
    for<'a> FileBrowserIo<'a>: Global<'a, W>,
{
    type Window = W;
    type Action = Action;
    type Effect = Outcome;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "file_browser.wire", skip_all)
    )]
    fn wire(&self, window: &W) {
        let io = window.global::<FileBrowserIo>();
        connect!(io, self.queue, on_entry_pressed, (index) => Action::Press(index as usize));
        connect!(io, self.queue, on_up_pressed, () => Action::Up);
        connect!(io, self.queue, on_select_folder, () => Action::SelectFolder);
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "file_browser.sync",
            level = "trace",
            skip_all,
            fields(open = self.state.is_some())
        )
    )]
    fn sync(&mut self, window: &W, _context: &AppContext) {
        let io = window.global::<FileBrowserIo>();
        let Some(state) = &self.state else {
            io.set_open(false);
            io.set_page(0);
            return;
        };
        let path = state.path.display().to_string();
        // A new directory starts on its first page.
        if io.get_path().as_str() != path {
            io.set_page(0);
        }
        let entries = state
            .entries
            .iter()
            .map(|entry| PagedLine {
                label: entry.label.as_str().into(),
                value: Default::default(),
                selected: false,
            })
            .collect();
        if let Some(model) = changed_model(&io.get_entries(), entries) {
            io.set_entries(model);
        }
        io.set_open(true);
        io.set_path(path.into());
        io.set_can_select_folder(state.mode == Mode::Directory);
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "file_browser.apply", skip_all, fields(action = ?action))
    )]
    async fn apply(&mut self, action: Action, _env: &mut Env<'_>) -> Outcome {
        match action {
            Action::Press(index) => self.press(index).await,
            Action::Up => self.up().await,
            Action::SelectFolder => match self.state.as_ref().map(|state| state.path.clone()) {
                Some(path) => self.choose(path),
                None => Outcome::Idle,
            },
            Action::Dismissed => {
                self.close();
                Outcome::Dismissed
            }
        }
    }
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(name = "file_browser.list", fields(path = %path.display(), ?mode))
)]
async fn list_files(path: &Path, mode: Mode) -> Vec<Entry> {
    let mut listed = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(path).await {
        Ok(read_dir) => read_dir,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "file browser could not list a directory"
            );
            return Vec::new();
        }
    };
    while let Some(entry) = crate::fs::next_dir_entry(&mut read_dir).await {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        // A symlink needs the stat that follows it to know what it points at.
        let is_dir = if file_type.is_symlink() {
            entry
                .metadata()
                .await
                .is_ok_and(|metadata| metadata.is_dir())
        } else {
            file_type.is_dir()
        };
        if mode == Mode::Directory && !is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        listed.push((entry.path(), name, is_dir));
    }
    listed.sort_by_cached_key(|(_, name, is_dir)| (Reverse(*is_dir), name.to_lowercase()));
    listed
        .into_iter()
        .map(|(path, name, is_dir)| Entry {
            path,
            label: if is_dir { format!("{name}/") } else { name },
            is_dir,
        })
        .collect()
}
