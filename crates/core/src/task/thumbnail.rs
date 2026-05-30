//! Background task that extracts book cover thumbnails.

use std::sync::mpsc::Sender;

use crate::db::Database;
use crate::device::CURRENT_DEVICE;
use crate::document::open;
use crate::library::Library;
use crate::settings::Settings;
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::unit::scale_by_dpi;
use crate::view::BIG_BAR_HEIGHT;
use crate::view::Event;

/// Runs thumbnail extraction for missing book previews in a library (or all libraries when `library_index` is `None`).
pub struct ThumbnailExtractionTask {
    database: Database,
    settings: Settings,
    /// Which library to process. `None` means all configured libraries.
    library_index: Option<usize>,
}

impl ThumbnailExtractionTask {
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
                tracing::error!(error = %e, library_index = index, "failed to open library for thumbnail extraction");
                return;
            }
        };

        let books = match library.db.books_without_thumbnails(library.library_id) {
            Ok(books) => books,
            Err(e) => {
                tracing::error!(error = %e, library_id = library.library_id, "failed to query books without thumbnails");
                return;
            }
        };

        if books.is_empty() {
            tracing::debug!(
                library_id = library.library_id,
                "no missing thumbnails for library"
            );
            return;
        }

        tracing::info!(
            library_id = library.library_id,
            count = books.len(),
            "starting thumbnail extraction for library"
        );

        let dpi = CURRENT_DEVICE.dpi;
        let big_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        let th = big_height;
        let tw = 3 * th / 4;

        for (fp, path) in books {
            if shutdown.should_stop() {
                tracing::info!("thumbnail extraction task shutdown requested, stopping");
                return;
            }

            let full_path = library.home.join(&path);
            tracing::debug!(path = %path.display(), "extracting thumbnail");

            match open(&full_path)
                .and_then(|mut doc| {
                    doc.preview_pixmap(tw as f32, th as f32, CURRENT_DEVICE.color_samples())
                })
                .and_then(|pixmap| pixmap.to_png_bytes().ok())
            {
                Some(bytes) => {
                    if let Err(e) = library.db.save_thumbnail(fp, &bytes) {
                        tracing::error!(error = %e, path = %path.display(), "failed to save thumbnail to database");
                    } else {
                        hub.send(Event::RefreshBookPreview(path)).ok();
                    }
                }
                None => {
                    tracing::warn!(path = %path.display(), "failed to extract preview for book");
                }
            }
        }
    }
}

impl BackgroundTask for ThumbnailExtractionTask {
    fn id(&self) -> TaskId {
        TaskId::ThumbnailExtraction
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
    }

    fn finished_event(&self) -> Option<Event> {
        Some(Event::ThumbnailExtractionFinished {
            library_index: self.library_index,
        })
    }
}
