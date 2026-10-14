//! Reusable Slint screens shared across windows.
//!
//! The markup for each lives under `ui/widgets/`, with the `*Io` global that
//! drives it in `ui/widgets/overlays.slint`. The types here are generic over
//! the window that draws them: any window whose component uses that overlay
//! exports its `*Io` global, and Slint then generates the global's accessor for
//! that window. The option list is also generic over the row value. A screen
//! constructs `ChoiceSheet<ItsRow, ItsWindow>` (and `FileBrowser<ItsWindow>`,
//! `TextSheet<ItsWindow>`, `ConfirmSheet<ItsWindow>`) and remembers why it
//! opened each overlay.

use std::marker::PhantomData;

pub mod choice_sheet;
pub mod confirm_sheet;
pub mod file_browser;
pub mod text_sheet;

/// Names the window an overlay binds to.
///
/// The overlay does not store a window. [`crate::view::slint::Component::wire`]
/// and [`crate::view::slint::Component::sync`] receive `&W`. This marker is
/// what puts that window in the overlay's type, so a screen can construct
/// `ChoiceSheet<ItsRow, ItsWindow>`.
pub type WindowOf<W> = PhantomData<W>;
