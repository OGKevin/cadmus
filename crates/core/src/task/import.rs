//! Background task that imports library contents from disk.

use std::sync::mpsc::Sender;

use crate::db::Database;
use crate::library::importer;
use crate::library::Library;
use crate::settings::Settings;
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::view::{Event, ViewId, ID_FEEDER};

/// Runs a full import for one library (or all libraries when `library_index` is `None`).
pub struct ImportTask {
    database: Database,
    settings: Settings,
    /// Which library to import. `None` means all configured libraries.
    library_index: Option<usize>,
}

impl ImportTask {
    pub fn new(database: Database, settings: Settings, library_index: Option<usize>) -> Self {
        Self {
            database,
            settings,
            library_index,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(hub, shutdown, self)))]
    fn run_for_index(&self, index: usize, hub: &Sender<Event>, shutdown: &ShutdownSignal) {
        let lib_settings = match self.settings.libraries.get(index) {
            Some(s) => s,
            None => {
                tracing::warn!(
                    library_index = index,
                    "library index out of range, skipping"
                );
                return;
            }
        };

        let library = match Library::new(&lib_settings.path, &self.database, &lib_settings.name) {
            Ok(lib) => lib,
            Err(e) => {
                tracing::error!(error = %e, library_index = index, "failed to open library for import");
                return;
            }
        };

        let notif_id = ViewId::MessageNotif(ID_FEEDER.next());
        importer::run(
            &library.db,
            library.library_id,
            &library.home,
            &self.settings.import,
            hub,
            notif_id,
            shutdown,
        );
    }
}

impl BackgroundTask for ImportTask {
    fn id(&self) -> TaskId {
        TaskId::Import
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn run(&mut self, hub: &Sender<Event>, shutdown: &ShutdownSignal) {
        match self.library_index {
            Some(index) => {
                self.run_for_index(index, hub, shutdown);
            }
            None => {
                for index in 0..self.settings.libraries.len() {
                    if shutdown.should_stop() {
                        return;
                    }
                    self.run_for_index(index, hub, shutdown);
                }
            }
        }

        if !shutdown.should_stop() {
            hub.send(Event::ImportFinished {
                library_index: self.library_index,
            })
            .ok();
        }
    }
}
