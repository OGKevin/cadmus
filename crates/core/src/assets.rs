//! Embedded documentation assets.
//!
//! This module provides access to documentation files embedded in the binary
//! at compile time using rust-embed.

use rust_embed::Embed;
use rust_embed::EmbeddedFile;

#[cfg(debug_assertions)]
use std::sync::OnceLock;

/// Cached documentation bytes to prevent repeated memory leaks in debug builds.
///
/// In debug builds, rust-embed reads files from disk and returns `Cow::Owned`,
/// requiring us to leak the data to get a 'static reference. This cache ensures
/// the leak only happens once.
#[cfg(debug_assertions)]
static DOCUMENTATION_CACHE: OnceLock<&'static [u8]> = OnceLock::new();

/// Embedded documentation EPUB file.
///
/// Contains the Cadmus documentation EPUB file generated from mdbook.
/// Only the EPUB file is embedded, not the entire folder.
///
/// # Example
///
/// ```
/// use cadmus_core::assets::DocumentationAssets;
///
/// let epub = DocumentationAssets::get_documentation();
/// assert!(!epub.data.is_empty());
/// ```
#[derive(Embed)]
#[folder = "../../docs/book/epub/"]
#[include = "Cadmus Documentation.epub"]
pub struct DocumentationAssets;

impl DocumentationAssets {
    /// Returns the embedded documentation EPUB file.
    ///
    /// # Panics
    ///
    /// Panics if the documentation EPUB file is not found in embedded assets.
    /// This should never happen in a properly built binary.
    ///
    /// # Example
    ///
    /// ```
    /// use cadmus_core::assets::DocumentationAssets;
    ///
    /// let epub = DocumentationAssets::get_documentation();
    /// // The EPUB data is available as a byte slice
    /// let data: &[u8] = &epub.data;
    /// ```
    pub fn get_documentation() -> EmbeddedFile {
        Self::get("Cadmus Documentation.epub")
            .expect("Documentation EPUB not found in embedded assets")
    }
}

/// Opens the embedded documentation in a Reader view.
///
/// This helper function is shared between the app and emulator to avoid code duplication.
/// It retrieves the embedded EPUB, creates a Reader, and returns it.
///
/// The EPUB data is accessed without copying (zero-copy). In release builds, the data
/// is embedded as a static reference. In debug builds, the data is loaded from disk
/// and leaked to obtain a static reference, which is acceptable since the documentation
/// is loaded once and lives for the entire program duration.
///
/// # Arguments
///
/// * `rect` - The rectangle defining the display area for the reader
/// * `hub` - The event hub for sending update events
/// * `context` - The application context containing display settings and fonts
///
/// # Returns
///
/// Returns `Some(Reader)` if the documentation was successfully opened,
/// or `None` if there was an error parsing the EPUB.
///
/// # Example
///
/// ```no_run
/// use cadmus_core::assets::open_documentation;
/// use cadmus_core::context::Context;
/// use cadmus_core::view::Hub;
/// use cadmus_core::geom::Rectangle;
/// use std::sync::mpsc::channel;
///
/// // Note: In actual use, context and hub are provided by the application.
/// // This example shows the API pattern.
/// # fn example(rect: Rectangle, hub: &Hub, context: &mut Context) {
/// if let Some(reader) = open_documentation(rect, hub, context) {
///     // Documentation opened successfully
///     // The reader can be used to display the embedded EPUB
/// }
/// # }
/// ```
pub fn open_documentation(
    rect: crate::geom::Rectangle,
    hub: &crate::view::Hub,
    context: &mut crate::context::Context,
) -> Option<crate::view::reader::Reader> {
    #[cfg(debug_assertions)]
    let static_bytes = DOCUMENTATION_CACHE.get_or_init(|| {
        let epub_file = DocumentationAssets::get_documentation();
        match epub_file.data {
            std::borrow::Cow::Borrowed(bytes) => bytes,
            std::borrow::Cow::Owned(vec) => Box::leak(vec.into_boxed_slice()),
        }
    });

    #[cfg(not(debug_assertions))]
    let static_bytes = {
        let epub_file = DocumentationAssets::get_documentation();
        match epub_file.data {
            std::borrow::Cow::Borrowed(bytes) => bytes,
            std::borrow::Cow::Owned(_) => {
                unreachable!("Owned data should not occur in release builds")
            }
        }
    };

    crate::view::reader::Reader::from_embedded_epub(rect, static_bytes, hub, context)
}
