//! Shared text dialog behind `TextSheet`.
//!
//! The component (`ui/widgets/text_sheet.slint`) draws a field floating above
//! a keyboard docked to the bottom of the window. This type owns the string
//! and the caret. Keys report insertions, deletions, and motion. The key
//! caps come from the selected keyboard layout. Holding a character key
//! offers the accented forms of that character. The type parameter is the
//! window that draws it: any window whose component exports `TextSheetIo`.

use slint::{ComponentHandle, Global, ModelRc, VecModel};

use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::DeviceIdentity as _;
use crate::fl;
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::view::keyboard::{
    FaceAction, KeyFace, ModifierState, key_faces, reader_keyboard_span, selected_layout,
    variants_for,
};
use crate::view::slint::component::{Component, Env};
use crate::view::slint::generated::{KeyAction, KeyCap, TextSheetIo};
use crate::view::slint::{CallbackQueue, changed_model, connect};

/// One callback from the text dialog.
#[derive(Debug)]
pub(in crate::view) enum Action {
    /// Insert `text` at the caret. Extra characters past the field's limit
    /// are dropped.
    Insert(String),
    /// Drop the character before the caret.
    DeleteBack,
    /// Drop the character after the caret.
    DeleteForward,
    MoveBack,
    MoveForward,
    MoveStart,
    MoveEnd,
    Shift,
    Alternate,
    /// The reader held `text`'s character. Open its accented forms.
    Held(String),
    /// Close the accent strip without inserting.
    PopupDismissed,
    Accepted,
    Dismissed,
}

/// How the host queues one captured frame while this dialog may be open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::view) struct FrameRefresh {
    pub(in crate::view) mode: UpdateMode,
    /// When false, the update does not wait for the previous waveform.
    pub(in crate::view) wait: bool,
}

/// Whether a finger-down or finger-move frame should be left unqueued.
///
/// A finger-down on an open keyboard is the black key flash, so the host
/// queues it. Any other press preview waits for the release, which carries
/// the new state.
pub(in crate::view) fn skip_press_frame(
    keyboard_open: bool,
    finger_down: bool,
    finger_preview: bool,
) -> bool {
    let flash = keyboard_open && finger_down;
    finger_preview && !flash
}

/// An open keyboard does not wait on the previous waveform. A finger-down
/// on that keyboard uses [`UpdateMode::Fast`], the same as the reader
/// keyboard's pressed key. Every other frame uses [`UpdateMode::Gui`].
pub(in crate::view) fn frame_refresh(keyboard_open: bool, finger_down: bool) -> FrameRefresh {
    if keyboard_open && finger_down {
        FrameRefresh {
            mode: UpdateMode::Fast,
            wait: false,
        }
    } else if keyboard_open {
        FrameRefresh {
            mode: UpdateMode::Gui,
            wait: false,
        }
    } else {
        FrameRefresh {
            mode: UpdateMode::Gui,
            wait: true,
        }
    }
}

/// What the screen does after one callback.
pub(in crate::view) enum Outcome {
    /// Nothing further.
    Idle,
    /// The field, the caret, the layer, or the accent strip changed.
    Changed,
    /// The reader accepted the field. The dialog is closed.
    Accepted(String),
    /// The reader closed the dialog.
    Dismissed,
}

/// State of one open text dialog.
pub(in crate::view) struct TextSheet<W> {
    pending: Option<Pending>,
    shift: ModifierState,
    alternate: ModifierState,
    popup: Vec<String>,
    /// Layout name and modifiers last copied into the window.
    rendered: Option<(String, ModifierState, ModifierState)>,
    /// Label, text, and caret last copied into the window.
    copied: Option<(String, String, usize)>,
    queue: CallbackQueue<Action>,
    window: super::WindowOf<W>,
}

struct Pending {
    label: String,
    text: String,
    /// Byte index into `text`, always on a character boundary.
    cursor: usize,
    max_chars: usize,
}

impl<W> TextSheet<W> {
    pub(in crate::view) fn new() -> Self {
        Self {
            pending: None,
            shift: ModifierState::Off,
            alternate: ModifierState::Off,
            popup: Vec::new(),
            rendered: None,
            copied: None,
            queue: CallbackQueue::new(),
            window: super::WindowOf::default(),
        }
    }

    /// Opens the dialog on `label`, editing `initial`.
    ///
    /// `numbers` locks the alternate layer, which is the layout's digits.
    /// `max_chars` is how many characters the field keeps. The caret starts
    /// after `initial`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "text_sheet.open",
            skip(self, label, initial),
            fields(numbers = numbers, max_chars = max_chars, len = initial.chars().count())
        )
    )]
    pub(in crate::view) fn open(
        &mut self,
        label: String,
        initial: String,
        numbers: bool,
        max_chars: usize,
    ) {
        tracing::debug!("text dialog opened");
        let cursor = initial.len();
        self.pending = Some(Pending {
            label,
            text: initial,
            cursor,
            max_chars,
        });
        self.shift = ModifierState::Off;
        self.alternate = if numbers {
            ModifierState::Locked
        } else {
            ModifierState::Off
        };
        self.popup.clear();
        self.rendered = None;
        self.copied = None;
    }

    /// Closes the dialog.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "text_sheet.close", skip_all)
    )]
    pub(in crate::view) fn close(&mut self) {
        if self.pending.is_none() {
            return;
        }
        tracing::debug!("text dialog closed");
        self.pending = None;
        self.reset_keys();
    }

    pub(in crate::view) fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    fn reset_keys(&mut self) {
        self.shift = ModifierState::Off;
        self.alternate = ModifierState::Off;
        self.popup.clear();
        self.rendered = None;
        self.copied = None;
    }

    fn clear_popup(&mut self) -> bool {
        if self.popup.is_empty() {
            false
        } else {
            self.popup.clear();
            true
        }
    }

    /// Releases a one-shot shift or alternate. Locked layers stay.
    fn release_modifiers(&mut self) -> bool {
        let shift = self.shift;
        let alternate = self.alternate;
        self.shift = shift.release();
        self.alternate = alternate.release();
        shift != self.shift || alternate != self.alternate
    }

    fn edit(&mut self, change: impl FnOnce(&mut Pending) -> bool) -> Outcome {
        let Some(pending) = self.pending.as_mut() else {
            return Outcome::Idle;
        };
        let changed = change(pending);
        let cleared = self.clear_popup();
        if changed || cleared {
            Outcome::Changed
        } else {
            Outcome::Idle
        }
    }

    fn handle(&mut self, action: Action) -> Outcome {
        match action {
            Action::Insert(extra) => {
                let Some(pending) = self.pending.as_mut() else {
                    return Outcome::Idle;
                };
                let inserted = pending.push(&extra);
                let released = if extra == " " {
                    false
                } else {
                    self.release_modifiers()
                };
                let cleared = self.clear_popup();
                if inserted || released || cleared {
                    if inserted {
                        let len = self
                            .pending
                            .as_ref()
                            .map_or(0, |pending| pending.text.chars().count());
                        tracing::debug!(len, "text dialog insert");
                    }
                    Outcome::Changed
                } else {
                    Outcome::Idle
                }
            }
            Action::DeleteBack => self.edit(Pending::delete_back),
            Action::DeleteForward => self.edit(Pending::delete_forward),
            Action::MoveBack => self.edit(Pending::move_back),
            Action::MoveForward => self.edit(Pending::move_forward),
            Action::MoveStart => self.edit(Pending::move_start),
            Action::MoveEnd => self.edit(Pending::move_end),
            Action::Shift => {
                if self.pending.is_none() {
                    return Outcome::Idle;
                }
                self.shift = self.shift.cycle();
                self.clear_popup();
                Outcome::Changed
            }
            Action::Alternate => {
                if self.pending.is_none() {
                    return Outcome::Idle;
                }
                self.alternate = self.alternate.cycle();
                self.clear_popup();
                Outcome::Changed
            }
            Action::Held(text) => self.hold(&text),
            Action::PopupDismissed => {
                if self.clear_popup() {
                    Outcome::Changed
                } else {
                    Outcome::Idle
                }
            }
            Action::Accepted => match self.pending.take() {
                Some(pending) => {
                    tracing::debug!(len = pending.text.chars().count(), "text dialog accepted");
                    self.reset_keys();
                    Outcome::Accepted(pending.text)
                }
                None => Outcome::Idle,
            },
            Action::Dismissed => {
                self.close();
                Outcome::Dismissed
            }
        }
    }

    fn hold(&mut self, text: &str) -> Outcome {
        if self.pending.is_none() {
            return Outcome::Idle;
        }
        let mut chars = text.chars();
        let Some(base) = chars.next() else {
            return Outcome::Idle;
        };
        if chars.next().is_some() {
            return Outcome::Idle;
        }
        let variants = variants_for(base);
        if variants.is_empty() {
            return Outcome::Idle;
        }
        self.popup = variants.into_iter().map(|ch| ch.to_string()).collect();
        tracing::debug!(count = self.popup.len(), "text dialog accents");
        Outcome::Changed
    }

    fn popup_model(&self) -> Vec<slint::SharedString> {
        self.popup.iter().map(|ch| ch.as_str().into()).collect()
    }
}

impl Pending {
    /// Inserts what fits under `max_chars` at the caret.
    fn push(&mut self, extra: &str) -> bool {
        let room = self.max_chars.saturating_sub(self.text.chars().count());
        let extra: String = extra.chars().take(room).collect();
        if extra.is_empty() {
            return false;
        }
        self.text.insert_str(self.cursor, &extra);
        self.cursor += extra.len();
        true
    }

    fn delete_back(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let Some(previous) = self.text[..self.cursor].chars().next_back() else {
            return false;
        };
        let start = self.cursor - previous.len_utf8();
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        true
    }

    fn delete_forward(&mut self) -> bool {
        let Some(next) = self.text[self.cursor..].chars().next() else {
            return false;
        };
        let end = self.cursor + next.len_utf8();
        self.text.replace_range(self.cursor..end, "");
        true
    }

    fn move_back(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let Some(previous) = self.text[..self.cursor].chars().next_back() else {
            return false;
        };
        self.cursor -= previous.len_utf8();
        true
    }

    fn move_forward(&mut self) -> bool {
        let Some(next) = self.text[self.cursor..].chars().next() else {
            return false;
        };
        self.cursor += next.len_utf8();
        true
    }

    fn move_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor = 0;
        true
    }

    fn move_end(&mut self) -> bool {
        if self.cursor == self.text.len() {
            return false;
        }
        self.cursor = self.text.len();
        true
    }
}

fn move_action(action: KeyAction) -> Action {
    match action {
        KeyAction::MoveBack => Action::MoveBack,
        KeyAction::MoveForward => Action::MoveForward,
        KeyAction::MoveStart => Action::MoveStart,
        KeyAction::MoveEnd => Action::MoveEnd,
        _ => Action::PopupDismissed,
    }
}

fn key_cap(face: KeyFace) -> KeyCap {
    KeyCap {
        label: face.label.into(),
        insert: face.insert.into(),
        stretch: face.stretch,
        engaged: face.engaged,
        compact: face.compact,
        holdable: face.holdable,
        action: match face.action {
            FaceAction::Character => KeyAction::Character,
            FaceAction::DeleteBack => KeyAction::DeleteBack,
            FaceAction::DeleteForward => KeyAction::DeleteForward,
            FaceAction::Shift => KeyAction::Shift,
            FaceAction::Alternate => KeyAction::Alternate,
            FaceAction::Submit => KeyAction::Submit,
            FaceAction::MoveBack => KeyAction::MoveBack,
            FaceAction::MoveForward => KeyAction::MoveForward,
            FaceAction::MoveStart => KeyAction::MoveStart,
            FaceAction::MoveEnd => KeyAction::MoveEnd,
            FaceAction::Dismiss => KeyAction::Dismiss,
            FaceAction::Ignore => KeyAction::Ignore,
        },
    }
}

fn faces_model(faces: &[Vec<KeyFace>]) -> ModelRc<ModelRc<KeyCap>> {
    let rows = faces
        .iter()
        .map(|row| {
            let keys = row.iter().cloned().map(key_cap).collect::<Vec<_>>();
            ModelRc::new(VecModel::from(keys))
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

#[async_trait::async_trait(?Send)]
impl<W> Component for TextSheet<W>
where
    W: ComponentHandle + 'static,
    for<'a> TextSheetIo<'a>: Global<'a, W>,
{
    type Window = W;
    type Action = Action;
    type Effect = Outcome;

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "text_sheet.wire", skip_all)
    )]
    fn wire(&self, window: &W) {
        let io = window.global::<TextSheetIo>();
        connect!(io, self.queue, on_typed, (text) => Action::Insert(text.to_string()));
        connect!(io, self.queue, on_deleted, () => Action::DeleteBack);
        connect!(io, self.queue, on_deleted_forward, () => Action::DeleteForward);
        connect!(io, self.queue, on_moved, (action) => move_action(action));
        connect!(io, self.queue, on_shift_pressed, () => Action::Shift);
        connect!(io, self.queue, on_alternate_pressed, () => Action::Alternate);
        connect!(io, self.queue, on_held, (text) => Action::Held(text.to_string()));
        connect!(io, self.queue, on_popup_dismissed, () => Action::PopupDismissed);
        connect!(io, self.queue, on_accepted, () => Action::Accepted);
        connect!(io, self.queue, on_dismissed, () => Action::Dismissed);
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            name = "text_sheet.sync",
            level = "trace",
            skip_all,
            fields(open = self.pending.is_some())
        )
    )]
    fn sync(&mut self, window: &W, context: &AppContext) {
        let io = window.global::<TextSheetIo>();
        let Some(pending) = self.pending.as_ref() else {
            io.set_open(false);
            if let Some(model) = changed_model(&io.get_variants(), self.popup_model()) {
                io.set_variants(model);
            }
            return;
        };
        let same_field = self.copied.as_ref().is_some_and(|(label, text, cursor)| {
            label == &pending.label && text == &pending.text && *cursor == pending.cursor
        });
        if !same_field {
            let label = pending.label.clone();
            let text = pending.text.clone();
            let cursor = pending.cursor;
            io.set_label(label.as_str().into());
            io.set_text_before(text[..cursor].into());
            io.set_text_after(text[cursor..].into());
            self.copied = Some((label, text, cursor));
        }
        io.set_cancel_label(fl!("cancel").into());
        io.set_accept_label(fl!("ok").into());
        io.set_open(true);

        let layout_name = context.settings.keyboard_layout.as_str();
        let shift = self.shift;
        let alternate = self.alternate;
        let same_keys =
            self.rendered
                .as_ref()
                .is_some_and(|(name, rendered_shift, rendered_alternate)| {
                    name == layout_name
                        && *rendered_shift == shift
                        && *rendered_alternate == alternate
                });
        if !same_keys {
            let layout = selected_layout(context);
            let span = reader_keyboard_span(
                context.device.framebuffer().width(),
                context.device.dpi(),
                &layout,
            );
            io.set_block_height(span.block_height);
            io.set_key_height(span.key_height);
            io.set_key_gap(span.key_gap);
            io.set_rows(faces_model(&key_faces(&layout, shift, alternate)));
            self.rendered = Some((layout_name.to_owned(), shift, alternate));
        }
        if let Some(model) = changed_model(&io.get_variants(), self.popup_model()) {
            io.set_variants(model);
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(name = "text_sheet.apply", skip_all, fields(action = action_name(&action)))
    )]
    async fn apply(&mut self, action: Action, _env: &mut Env<'_>) -> Outcome {
        self.handle(action)
    }
}

#[cfg(feature = "tracing")]
fn action_name(action: &Action) -> &'static str {
    match action {
        Action::Insert(_) => "insert",
        Action::DeleteBack => "delete-back",
        Action::DeleteForward => "delete-forward",
        Action::MoveBack => "move-back",
        Action::MoveForward => "move-forward",
        Action::MoveStart => "move-start",
        Action::MoveEnd => "move-end",
        Action::Shift => "shift",
        Action::Alternate => "alternate",
        Action::Held(_) => "held",
        Action::PopupDismissed => "popup-dismissed",
        Action::Accepted => "accepted",
        Action::Dismissed => "dismissed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::framebuffer::UpdateMode;

    fn pending(text: &str, max_chars: usize) -> Pending {
        Pending {
            label: String::new(),
            text: text.to_owned(),
            cursor: text.len(),
            max_chars,
        }
    }

    #[test]
    fn insert_stops_at_the_field_limit() {
        let mut field = pending("12", 3);
        assert!(field.push("3"));
        assert_eq!(field.text, "123");
        assert!(!field.push("4"));
        assert_eq!(field.text, "123");

        let mut field = pending("é", 2);
        assert!(field.push("e"));
        assert_eq!(field.text, "ée");
        assert!(!field.push("x"));
        assert_eq!(field.text, "ée");
    }

    #[test]
    fn insert_keeps_only_what_fits() {
        let mut field = pending("1", 3);
        assert!(field.push("2345"));
        assert_eq!(field.text, "123");
        assert_eq!(field.cursor, 3);
    }

    #[test]
    fn insert_and_delete_follow_the_caret() {
        let mut field = pending("ac", 8);
        field.cursor = 1;
        assert!(field.push("b"));
        assert_eq!(field.text, "abc");
        assert_eq!(field.cursor, 2);
        assert!(field.delete_forward());
        assert_eq!(field.text, "ab");
        assert!(field.move_back());
        assert!(field.delete_forward());
        assert_eq!(field.text, "a");
        assert_eq!(field.cursor, 1);
        assert!(field.move_back());
        assert_eq!(field.cursor, 0);
        assert!(!field.move_back());
        assert!(!field.delete_back());
    }

    #[test]
    fn delete_drops_one_character() {
        let mut field = pending("ab", 3);
        assert!(field.delete_back());
        assert_eq!(field.text, "a");

        let mut field = pending("é", 3);
        assert!(field.delete_back());
        assert_eq!(field.text, "");
        assert!(!field.delete_back());
    }

    #[test]
    fn holding_e_offers_acute_and_insert_clears_the_strip() {
        let mut sheet = TextSheet::<()>::new();
        sheet.open("Name".into(), String::new(), false, 8);
        assert!(matches!(
            sheet.handle(Action::Held("e".into())),
            Outcome::Changed
        ));
        assert!(sheet.popup.iter().any(|ch| ch == "é"));
        assert!(matches!(
            sheet.handle(Action::Insert("é".into())),
            Outcome::Changed
        ));
        let pending = sheet.pending.as_ref().expect("still open");
        assert_eq!(pending.text, "é");
        assert!(sheet.popup.is_empty());
        assert_eq!(sheet.shift, ModifierState::Off);
    }

    #[test]
    fn a_space_does_not_release_one_shot_shift() {
        let mut sheet = TextSheet::<()>::new();
        sheet.open("Name".into(), String::new(), false, 8);
        assert!(matches!(sheet.handle(Action::Shift), Outcome::Changed));
        assert_eq!(sheet.shift, ModifierState::Once);
        sheet.handle(Action::Insert(" ".into()));
        assert_eq!(sheet.shift, ModifierState::Once);
        sheet.handle(Action::Insert("A".into()));
        assert_eq!(sheet.shift, ModifierState::Off);
        assert_eq!(sheet.pending.as_ref().expect("open").text, " A");
    }

    #[test]
    fn an_open_keyboard_flashes_a_finger_down_and_does_not_wait() {
        let closed = frame_refresh(false, true);
        assert_eq!(closed.mode, UpdateMode::Gui);
        assert!(closed.wait);
        assert!(skip_press_frame(false, true, true));

        let flash = frame_refresh(true, true);
        assert_eq!(flash.mode, UpdateMode::Fast);
        assert!(!flash.wait);
        assert!(!skip_press_frame(true, true, true));

        let motion = frame_refresh(true, false);
        assert_eq!(motion.mode, UpdateMode::Gui);
        assert!(!motion.wait);
        assert!(skip_press_frame(true, false, true));
    }

    #[test]
    fn numbers_open_on_the_locked_alternate_layer() {
        let mut sheet = TextSheet::<()>::new();
        sheet.open("Timeout".into(), "1".into(), true, 8);
        assert_eq!(sheet.alternate, ModifierState::Locked);
        assert_eq!(sheet.pending.as_ref().expect("open").cursor, 1);
    }
}
