//! Background task that imports library contents from disk.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db::Database;
use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
use crate::library::Library;
use crate::library::importer::{self, ImportOutcome};
use crate::settings::Settings;
use crate::task::{BackgroundTask, TaskFuture, TaskId};
use crate::view::{Event, ID_FEEDER, ViewId};
use tokio_util::sync::CancellationToken;

/// Runs an import for one library (or all libraries when `library_index` is `None`).
///
/// When `force` is `false` the import is incremental: files whose stored `mtime` and
/// `file_size` have not changed are skipped without re-fingerprinting. When `force` is
/// `true` every file is re-fingerprinted regardless.
pub struct ImportTask {
    database: Database,
    settings: Settings,
    /// Which library to import. `None` means all configured libraries.
    library_index: Option<usize>,
    /// When `true`, skip the mtime/size cache and re-fingerprint every file.
    force: bool,
    install_dir: PathBuf,
    inhibitor: Arc<Inhibitor>,
    outcome: ImportOutcome,
}

impl ImportTask {
    pub fn new(
        database: Database,
        settings: Settings,
        library_index: Option<usize>,
        force: bool,
        install_dir: impl Into<PathBuf>,
        inhibitor: Arc<Inhibitor>,
    ) -> Self {
        Self {
            database,
            settings,
            library_index,
            force,
            install_dir: install_dir.into(),
            inhibitor,
            outcome: ImportOutcome::Failed,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(hub, cancel, self)))]
    async fn run_for_index(
        &self,
        index: usize,
        hub: &crate::view::Hub,
        cancel: &CancellationToken,
    ) -> ImportOutcome {
        let lib_settings = match self.settings.libraries.get(index) {
            Some(s) => s,
            None => {
                tracing::warn!(
                    library_index = index,
                    "library index out of range, skipping"
                );
                return ImportOutcome::Failed;
            }
        };

        let library = match Library::new(&lib_settings.path, &self.database, &lib_settings.name)
            .await
        {
            Ok(lib) => lib,
            Err(e) => {
                tracing::error!(error = %e, library_index = index, "failed to open library for import");
                return ImportOutcome::Failed;
            }
        };

        let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        importer::run(
            &library.db,
            library.library_id,
            &library.home,
            &self.install_dir,
            &self.settings.import,
            self.force,
            hub,
            notif_id,
            cancel,
        )
        .await
    }
}

impl BackgroundTask for ImportTask {
    fn id(&self) -> TaskId {
        TaskId::Import
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            let _soft_suspend = match self
                .inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::LibraryImport)
            {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        soft_suspend_lease = %SoftSuspendName::LibraryImport,
                        "failed to acquire soft-suspend lease for library import task"
                    );
                    None
                }
            };
            match self.library_index {
                Some(index) => {
                    self.outcome = self.run_for_index(index, hub, cancel).await;
                }
                None => {
                    for index in 0..self.settings.libraries.len() {
                        if cancel.is_cancelled() {
                            self.outcome = ImportOutcome::Interrupted;
                            return;
                        }
                        match self.run_for_index(index, hub, cancel).await {
                            ImportOutcome::Completed => {}
                            failed_or_interrupted => {
                                self.outcome = failed_or_interrupted;
                                return;
                            }
                        }
                    }
                    self.outcome = ImportOutcome::Completed;
                }
            }
        })
    }

    fn finished_event(&self) -> Option<Event> {
        match self.outcome {
            ImportOutcome::Completed => Some(Event::ImportFinished {
                library_index: self.library_index,
            }),
            ImportOutcome::Failed => Some(Event::ImportFailed {
                library_index: self.library_index,
            }),
            ImportOutcome::Interrupted => None,
        }
    }
}
