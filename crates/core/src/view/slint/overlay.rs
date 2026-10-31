//! Overlay stack shared by Slint screens.

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::view::slint::component::{Component, Env};
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

/// A shared sheet and the control that opened it.
///
/// One option list, file browser, text dialog, and confirmation dialog serve
/// every control on a screen. The sheet reports what the reader picked; the
/// reason tells the screen which control asked. The reason belongs to the
/// sheet, so a screen does not keep a second list of what it is waiting for.
pub(in crate::view) struct Asked<K, S> {
    reason: Option<K>,
    sheet: S,
}

impl<K, S> Asked<K, S> {
    /// Wraps `sheet`. No control has opened it yet.
    pub(in crate::view) fn new(sheet: S) -> Self {
        Self {
            reason: None,
            sheet,
        }
    }

    /// The control that opened the sheet.
    pub(in crate::view) fn reason(&self) -> Option<&K> {
        self.reason.as_ref()
    }

    /// Names the control that opened the sheet, or forgets it.
    pub(in crate::view) fn set_reason(&mut self, reason: Option<K>) {
        self.reason = reason;
    }

    /// Takes the control that opened the sheet out of it.
    pub(in crate::view) fn take_reason(&mut self) -> Option<K> {
        self.reason.take()
    }

    /// The sheet, to read its state.
    pub(in crate::view) fn sheet(&self) -> &S {
        &self.sheet
    }

    /// The sheet, to open it.
    pub(in crate::view) fn sheet_mut(&mut self) -> &mut S {
        &mut self.sheet
    }
}

impl<K, S, W> Overlay<W> for Asked<K, S>
where
    W: ComponentHandle,
    S: Overlay<W>,
{
    fn is_open(&self) -> bool {
        self.sheet.is_open()
    }

    fn close(&mut self) {
        self.sheet.close();
        self.reason = None;
    }

    fn pages(&self) -> bool {
        self.sheet.pages()
    }

    fn bump(&mut self, window: &W, delta: i32) {
        self.sheet.bump(window, delta);
    }
}

#[async_trait::async_trait(?Send)]
impl<K, S> Component for Asked<K, S>
where
    S: Component,
{
    type Window = S::Window;
    type Action = S::Action;
    type Effect = S::Effect;

    fn wire(&self, window: &Self::Window) {
        self.sheet.wire(window);
    }

    fn take(&self) -> Vec<Self::Action> {
        self.sheet.take()
    }

    fn sync(&mut self, window: &Self::Window, context: &AppContext) {
        self.sheet.sync(window, context);
    }

    async fn apply(&mut self, action: Self::Action, env: &mut Env<'_>) -> Self::Effect {
        self.sheet.apply(action, env).await
    }
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
