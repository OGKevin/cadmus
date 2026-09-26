//! Background task that extracts book cover thumbnails.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db::Database;
use crate::device::inhibitor::{Inhibitor, Kind, SoftSuspendName};
use crate::document::open;
use crate::library::Library;
use crate::settings::Settings;
use crate::task::{BackgroundTask, TaskFuture, TaskId};
use crate::unit::scale_by_dpi;
use crate::view::BIG_BAR_HEIGHT;
use crate::view::Event;
use tokio_util::sync::CancellationToken;

/// Runs thumbnail extraction for missing book previews in a library (or all libraries when `library_index` is `None`).
pub struct ThumbnailExtractionTask {
    database: Database,
    settings: Settings,
    /// Which library to process. `None` means all configured libraries.
    library_index: Option<usize>,
    dpi: u16,
    color_samples: usize,
    install_dir: PathBuf,
    inhibitor: Arc<Inhibitor>,
}

impl ThumbnailExtractionTask {
    pub fn new(
        database: Database,
        settings: Settings,
        library_index: Option<usize>,
        dpi: u16,
        color_samples: usize,
        install_dir: impl Into<PathBuf>,
        inhibitor: Arc<Inhibitor>,
    ) -> Self {
        Self {
            database,
            settings,
            library_index,
            dpi,
            color_samples,
            install_dir: install_dir.into(),
            inhibitor,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(hub, cancel, self)))]
    async fn run_for_index(
        &self,
        index: usize,
        hub: &crate::view::Hub,
        cancel: &CancellationToken,
    ) {
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

        let library = match Library::new(&lib_settings.path, &self.database, &lib_settings.name)
            .await
        {
            Ok(lib) => lib,
            Err(e) => {
                tracing::error!(error = %e, library_index = index, "failed to open library for thumbnail extraction");
                return;
            }
        };

        let books = match library
            .db
            .books_without_thumbnails(library.library_id)
            .await
        {
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

        let dpi = self.dpi;
        let big_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        let th = big_height;
        let tw = 3 * th / 4;

        for (fp, path) in books {
            if cancel.is_cancelled() {
                tracing::info!("thumbnail extraction task shutdown requested, stopping");
                return;
            }

            let full_path = library.home.join(&path);
            tracing::debug!(path = %path.display(), "extracting thumbnail");

            let install_dir = self.install_dir.clone();
            let color_samples = self.color_samples;
            let rendered = tokio::task::spawn_blocking(move || {
                open(&full_path, &install_dir)
                    .and_then(|mut doc| doc.preview_pixmap(tw as f32, th as f32, color_samples))
                    .and_then(|pixmap| pixmap.to_png_bytes().ok())
            })
            .await;
            if cancel.is_cancelled() {
                tracing::info!("thumbnail extraction task shutdown requested, stopping");
                return;
            }
            match rendered {
                Ok(Some(bytes)) => {
                    if let Err(e) = library.db.save_thumbnail(fp, &bytes).await {
                        tracing::error!(error = %e, path = %path.display(), "failed to save thumbnail to database");
                    } else {
                        hub.send((Event::RefreshBookPreview(path)).into()).ok();
                    }
                }
                Ok(None) => {
                    tracing::warn!(path = %path.display(), "failed to extract preview for book");
                }
                Err(error) => {
                    tracing::error!(error = %error, path = %path.display(), "thumbnail render task panicked");
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
    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            let _soft_suspend = match self
                .inhibitor
                .acquire(Kind::SoftSuspend, SoftSuspendName::Thumbnail)
            {
                Ok(guard) => Some(guard),
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        soft_suspend_lease = %SoftSuspendName::Thumbnail,
                        "failed to acquire soft-suspend lease for thumbnail task"
                    );
                    None
                }
            };
            match self.library_index {
                Some(index) => {
                    self.run_for_index(index, hub, cancel).await;
                }
                None => {
                    for index in 0..self.settings.libraries.len() {
                        if cancel.is_cancelled() {
                            return;
                        }
                        self.run_for_index(index, hub, cancel).await;
                    }
                }
            }
        })
    }

    fn finished_event(&self) -> Option<Event> {
        Some(Event::ThumbnailExtractionFinished {
            library_index: self.library_index,
        })
    }
}
