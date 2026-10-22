//! The visible page of a list.
//!
//! The page belongs to the window that draws the list, so the hardware
//! page buttons and the list's own arrows move the same page.

use crate::view::slint::generated::{ChoiceSheetIo, FileBrowserIo, RefreshRatesIo};

/// A window whose list has a visible page.
pub(in crate::view) trait PagedIo {
    /// The visible page.
    fn visible_page(&self) -> i32;

    /// Sets the visible page.
    fn set_visible_page(&self, page: i32);

    /// Moves the visible page by `delta`.
    ///
    /// The page never goes below the first one. The list clamps it to the
    /// last one.
    fn bump(&self, delta: i32) {
        self.set_visible_page(self.visible_page().saturating_add(delta).max(0));
    }
}

impl<'a> PagedIo for ChoiceSheetIo<'a> {
    fn visible_page(&self) -> i32 {
        self.get_page()
    }

    fn set_visible_page(&self, page: i32) {
        self.set_page(page);
    }
}

impl<'a> PagedIo for FileBrowserIo<'a> {
    fn visible_page(&self) -> i32 {
        self.get_page()
    }

    fn set_visible_page(&self, page: i32) {
        self.set_page(page);
    }
}

impl<'a> PagedIo for RefreshRatesIo<'a> {
    fn visible_page(&self) -> i32 {
        self.get_page()
    }

    fn set_visible_page(&self, page: i32) {
        self.set_page(page);
    }
}
