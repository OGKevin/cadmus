//! Overlay stack shared by Slint screens.

use slint::{ComponentHandle, Global};

use crate::view::slint::components::choice_sheet::ChoiceSheet;
use crate::view::slint::components::confirm_sheet::ConfirmSheet;
use crate::view::slint::components::file_browser::FileBrowser;
use crate::view::slint::components::text_sheet::TextSheet;
use crate::view::slint::generated::{ChoiceSheetIo, ConfirmSheetIo, FileBrowserIo, TextSheetIo};

/// One sheet a screen can open on top of its body.
pub(in crate::view) trait Overlay<W: ComponentHandle> {
    fn is_open(&self) -> bool;
    fn close(&mut self);
    fn pages(&self) -> bool {
        false
    }
    fn bump(&mut self, _window: &W, _delta: i32) {}
}

/// Front-most first. Close and page-button walks use this order.
pub(in crate::view) struct OverlayStack<'a, W: ComponentHandle> {
    layers: Vec<&'a mut dyn Overlay<W>>,
}

impl<'a, W: ComponentHandle> OverlayStack<'a, W> {
    pub(in crate::view) fn new(layers: Vec<&'a mut dyn Overlay<W>>) -> Self {
        Self { layers }
    }

    pub(in crate::view) fn close_top(&mut self) -> bool {
        for layer in &mut self.layers {
            if layer.is_open() {
                layer.close();
                return true;
            }
        }
        false
    }

    pub(in crate::view) fn bump_open(&mut self, window: &W, delta: i32) -> bool {
        for layer in &mut self.layers {
            if layer.is_open() && layer.pages() {
                layer.bump(window, delta);
                return true;
            }
        }
        false
    }
}

impl<T, W> Overlay<W> for ChoiceSheet<T, W>
where
    W: ComponentHandle,
    for<'a> ChoiceSheetIo<'a>: Global<'a, W>,
{
    fn is_open(&self) -> bool {
        ChoiceSheet::is_open(self)
    }

    fn close(&mut self) {
        ChoiceSheet::close(self);
    }

    fn pages(&self) -> bool {
        true
    }

    fn bump(&mut self, window: &W, delta: i32) {
        ChoiceSheet::<T, W>::bump(window, delta);
    }
}

impl<W> Overlay<W> for ConfirmSheet<W>
where
    W: ComponentHandle,
    for<'a> ConfirmSheetIo<'a>: Global<'a, W>,
{
    fn is_open(&self) -> bool {
        ConfirmSheet::is_open(self)
    }

    fn close(&mut self) {
        ConfirmSheet::close(self);
    }
}

impl<W> Overlay<W> for FileBrowser<W>
where
    W: ComponentHandle,
    for<'a> FileBrowserIo<'a>: Global<'a, W>,
{
    fn is_open(&self) -> bool {
        FileBrowser::is_open(self)
    }

    fn close(&mut self) {
        FileBrowser::close(self);
    }

    fn pages(&self) -> bool {
        true
    }

    fn bump(&mut self, window: &W, delta: i32) {
        FileBrowser::<W>::bump(window, delta);
    }
}

impl<W> Overlay<W> for TextSheet<W>
where
    W: ComponentHandle,
    for<'a> TextSheetIo<'a>: Global<'a, W>,
{
    fn is_open(&self) -> bool {
        TextSheet::is_open(self)
    }

    fn close(&mut self) {
        TextSheet::close(self);
    }

    fn pages(&self) -> bool {
        true
    }
}
