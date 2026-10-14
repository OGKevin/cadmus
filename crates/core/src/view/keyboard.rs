use super::key::{Key, KeyKind};
use super::{BIG_BAR_HEIGHT, THICKNESS_MEDIUM};
use super::{
    Bus, EntryId, Event, Hub, ID_FEEDER, Id, KeyboardEvent, RenderData, RenderQueue, TextKind, View,
};
use crate::color::KEYBOARD_BG;
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::DeviceIdentity as _;
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::geom::{Dir, LinearDir, Rectangle};
use crate::gesture::GestureEvent;
use crate::input::DeviceEvent;
use crate::unit::scale_by_dpi;
use lazy_static::lazy_static;
use rustc_hash::FxHashMap;
use serde::Deserialize;

const PADDING_RATIO: f32 = 0.06;
/// A key at least this many units wide uses the long label.
const WIDE_KEY: f32 = 1.8;
/// Accent forms shown for one held key. The strip has no room for more.
const MAX_HOLD_VARIANTS: usize = 16;

/// Key size and gaps for a keyboard of `width` by `height`, in the same pixels.
struct RowMetrics {
    key_height: f32,
    padding: f32,
    rows_height: f32,
}

fn widest_row(layout: &Layout) -> f32 {
    layout
        .widths
        .iter()
        .map(|row| (row.len() + 1) as f32 * PADDING_RATIO + row.iter().sum::<f32>())
        .max_by(|a, b| a.partial_cmp(b).expect("Found NaNs"))
        .expect("Missing row widths")
}

fn row_metrics(width: f32, height: f32, layout: &Layout) -> RowMetrics {
    let rows_count = layout.keys.len() as f32;
    let key_height = (width / widest_row(layout))
        .min(height / (rows_count + PADDING_RATIO * (rows_count + 1.0)));
    let padding = PADDING_RATIO * key_height;
    let rows_height = key_height * rows_count + padding * (rows_count + 1.0);
    RowMetrics {
        key_height,
        padding,
        rows_height,
    }
}

/// The reader keyboard's key size and the height of its block, in the logical
/// pixels Slint uses. The block is what [`Keyboard`] keeps after it gives
/// unused whole bars back to the page.
pub(crate) struct KeyboardSpan {
    pub key_height: f32,
    pub key_gap: f32,
    pub block_height: f32,
}

/// Whole bars given back to the page, and the block that remains.
///
/// `allocated` and `bar` are in the same pixels. The reader reserves three
/// big bars for the keyboard, then returns every whole bar the keys do not use.
fn shrink_to_bars(allocated: i32, rows_height: f32, bar: i32) -> (i32, i32) {
    let bar = bar.max(1);
    let rows = rows_height.round().max(0.0) as i32;
    let gap = allocated.saturating_sub(rows) / bar;
    let returned = gap * bar;
    (returned, allocated - returned)
}

struct PlacedRows {
    key_height: f32,
    padding: f32,
    top: i32,
    start_y: f32,
}

/// Key size and the first row's top after unused bars are returned.
fn place_rows(rect: &Rectangle, layout: &Layout, dpi: u16) -> PlacedRows {
    let metrics = row_metrics(rect.width() as f32, rect.height() as f32, layout);
    let bar = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
    let (returned, block) = shrink_to_bars(rect.height() as i32, metrics.rows_height, bar);
    let top = rect.min.y + returned;
    let start_y = top as f32 + metrics.padding + (block as f32 - metrics.rows_height) / 2.0;
    PlacedRows {
        key_height: metrics.key_height,
        padding: metrics.padding,
        top,
        start_y,
    }
}

fn row_start_x(min_x: i32, width: u32, padding: f32, key_height: f32, widths: &[f32]) -> f32 {
    let row_width = (widths.len() + 1) as f32 * padding + widths.iter().sum::<f32>() * key_height;
    min_x as f32 + padding + (width as f32 - row_width) / 2.0
}

/// How tall the reader keyboard is for a screen `width` pixels wide at `dpi`.
///
/// The reader reserves three big bars, then [`shrink_to_bars`] gives unused
/// whole bars back to the page.
pub(crate) fn reader_keyboard_span(width: u32, dpi: u16, layout: &Layout) -> KeyboardSpan {
    let dpi = dpi.max(1);
    let big = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
    let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
    let allocated = 3 * big - thickness;
    let metrics = row_metrics(width as f32, allocated as f32, layout);
    let (_, block) = shrink_to_bars(allocated, metrics.rows_height, big);
    let logical = crate::unit::BASE_DPI / dpi as f32;
    KeyboardSpan {
        key_height: metrics.key_height * logical,
        key_gap: metrics.padding * logical,
        block_height: block as f32 * logical,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub name: String,
    pub outputs: [Vec<Vec<char>>; 4],
    pub keys: Vec<Vec<KeyKind>>,
    pub widths: Vec<Vec<f32>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ModifierState {
    #[default]
    Off,
    Once,
    Locked,
}

impl ModifierState {
    pub(crate) fn is_engaged(self) -> bool {
        !matches!(self, ModifierState::Off)
    }

    fn is_once(self) -> bool {
        matches!(self, ModifierState::Once)
    }

    pub(crate) fn is_locked(self) -> bool {
        matches!(self, ModifierState::Locked)
    }

    pub(crate) fn cycle(self) -> ModifierState {
        match self {
            ModifierState::Off => ModifierState::Once,
            ModifierState::Once => ModifierState::Locked,
            ModifierState::Locked => ModifierState::Off,
        }
    }

    /// Releases a pending single-press modifier, leaving locked ones engaged.
    pub(crate) fn release(self) -> ModifierState {
        if self.is_once() {
            ModifierState::Off
        } else {
            self
        }
    }
}

#[derive(Default, Debug)]
pub struct State {
    shift: ModifierState,
    alternate: ModifierState,
    combine: bool,
    control: ModifierState,
}

pub struct Keyboard {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    layout: Layout,
    state: State,
    combine_buffer: String,
}

impl Keyboard {
    pub fn new(rect: &mut Rectangle, number: bool, context: &mut AppContext) -> Keyboard {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let dpi = context.device.dpi();

        let layout = context.keyboard_layouts[&context.settings.keyboard_layout].clone();

        let mut state = State::default();

        if number {
            state.alternate = ModifierState::Locked;
        }

        let mut level = 0;

        if state.shift.is_engaged() {
            level += 1;
        }

        if state.alternate.is_engaged() {
            level += 2;
        }

        let placed = place_rows(rect, &layout, dpi);
        rect.min.y = placed.top;
        context.kb_rect = *rect;

        for (i, row) in layout.keys.iter().enumerate() {
            let y = placed.start_y + i as f32 * (placed.padding + placed.key_height);
            let start_x = row_start_x(
                rect.min.x,
                rect.width(),
                placed.padding,
                placed.key_height,
                &layout.widths[i],
            );
            let mut dx = 0.0;
            let mut dj = 0;

            for (j, kind) in row.iter().enumerate() {
                let key_width = layout.widths[i][j] * placed.key_height;
                let x = start_x + dx;
                dx += key_width + placed.padding;
                let key_rect = rect![
                    x.round() as i32,
                    y.round() as i32,
                    (x + key_width).round() as i32,
                    (y + placed.key_height).round() as i32
                ];
                let kind = match kind {
                    KeyKind::Output(c) if *c != ' ' => {
                        KeyKind::Output(layout.outputs[level][i][j - dj])
                    }
                    _ => {
                        dj = j + 1;
                        *kind
                    }
                };
                let mut key = Key::new(key_rect, kind);
                if number && kind == KeyKind::Alternate {
                    key.lock();
                }
                children.push(Box::new(key) as Box<dyn View>);
            }
        }

        Keyboard {
            id,
            rect: *rect,
            children,
            layout,
            state,
            combine_buffer: String::new(),
        }
    }

    fn update(&mut self, rq: &mut RenderQueue) {
        let mut level = 0;

        if self.state.shift.is_engaged() {
            level += 1;
        }

        if self.state.alternate.is_engaged() {
            level += 2;
        }

        let mut index = 0;

        for (i, row) in self.layout.keys.iter().enumerate() {
            let mut dj = 0;

            for (j, kind) in row.iter().enumerate() {
                if kind.is_variable_output() {
                    if let Some(child) = self.children[index].downcast_mut::<Key>() {
                        let ch = self.layout.outputs[level][i][j - dj];
                        child.update(KeyKind::Output(ch), rq);
                    }
                } else {
                    dj = j + 1;
                }
                index += 1;
            }
        }
    }

    fn release_modifiers(&mut self, rq: &mut RenderQueue) {
        if !self.state.shift.is_once()
            && !self.state.alternate.is_once()
            && !self.state.control.is_once()
        {
            return;
        }

        if self.state.shift.is_once() {
            self.state.shift = self.state.shift.release();
            self.release_key(KeyKind::Shift, rq);
        }

        if self.state.alternate.is_once() {
            self.state.alternate = self.state.alternate.release();
            self.release_key(KeyKind::Alternate, rq);
        }

        if self.state.control.is_once() {
            self.state.control = self.state.control.release();
            self.release_key(KeyKind::Control, rq);
        }

        self.update(rq);
    }

    fn release_key(&mut self, kind: KeyKind, rq: &mut RenderQueue) {
        if let Some(key) = self
            .children_mut()
            .iter_mut()
            .filter_map(|child| child.downcast_mut::<Key>())
            .find(|key| *key.kind() == kind)
        {
            key.release(rq);
        }
    }

    fn release_combine(&mut self, rq: &mut RenderQueue) {
        self.state.combine = false;
        self.combine_buffer.clear();
        for child in self.children_mut() {
            if let Some(key) = child.downcast_mut::<Key>() {
                if *key.kind() == KeyKind::Combine {
                    key.release(rq);
                    break;
                }
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl View for Keyboard {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    async fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::Key(k) => {
                match k {
                    KeyKind::Output(ch) => {
                        if self.state.control.is_engaged() && ch.is_ascii_alphabetic() {
                            hub.send((Event::Keyboard(KeyboardEvent::Control(ch))).into())
                                .ok();
                            self.release_modifiers(rq);
                        } else if self.state.combine {
                            self.combine_buffer.push(ch);
                            hub.send((Event::Keyboard(KeyboardEvent::Partial(ch))).into())
                                .ok();
                            if self.combine_buffer.len() > 1 {
                                if let Some(&ch) =
                                    DEFAULT_COMBINATIONS.get(&self.combine_buffer[..])
                                {
                                    hub.send((Event::Keyboard(KeyboardEvent::Append(ch))).into())
                                        .ok();
                                }
                                self.release_combine(rq);
                            }
                        } else {
                            hub.send((Event::Keyboard(KeyboardEvent::Append(ch))).into())
                                .ok();
                        }
                        if ch != ' ' {
                            self.release_modifiers(rq);
                        }
                    }
                    KeyKind::Shift => {
                        self.state.shift = self.state.shift.cycle();
                        if !self.state.shift.is_locked() {
                            self.update(rq);
                        }
                    }
                    KeyKind::Alternate => {
                        self.state.alternate = self.state.alternate.cycle();
                        if !self.state.alternate.is_locked() {
                            self.update(rq);
                        }
                    }
                    KeyKind::Delete(dir) => {
                        hub.send(
                            (Event::Keyboard(KeyboardEvent::Delete {
                                target: TextKind::Char,
                                dir,
                            }))
                            .into(),
                        )
                        .ok();
                    }
                    KeyKind::Move(dir) => {
                        hub.send(
                            (Event::Keyboard(KeyboardEvent::Move {
                                target: TextKind::Char,
                                dir,
                            }))
                            .into(),
                        )
                        .ok();
                    }
                    KeyKind::Combine => self.state.combine = !self.state.combine,
                    KeyKind::Return => {
                        self.release_combine(rq);
                        hub.send((Event::Keyboard(KeyboardEvent::Submit)).into())
                            .ok();
                    }
                    KeyKind::Tab => {
                        hub.send((Event::Keyboard(KeyboardEvent::Tab)).into()).ok();
                    }
                    KeyKind::Escape => {
                        hub.send((Event::Keyboard(KeyboardEvent::Escape)).into())
                            .ok();
                    }
                    KeyKind::ArrowUp => {
                        hub.send((Event::Keyboard(KeyboardEvent::Arrow(Dir::North))).into())
                            .ok();
                    }
                    KeyKind::ArrowDown => {
                        hub.send((Event::Keyboard(KeyboardEvent::Arrow(Dir::South))).into())
                            .ok();
                    }
                    KeyKind::ArrowRight => {
                        hub.send((Event::Keyboard(KeyboardEvent::Arrow(Dir::East))).into())
                            .ok();
                    }
                    KeyKind::ArrowLeft => {
                        hub.send((Event::Keyboard(KeyboardEvent::Arrow(Dir::West))).into())
                            .ok();
                    }
                    KeyKind::Control => {
                        self.state.control = self.state.control.cycle();
                        if !self.state.control.is_locked() {
                            self.update(rq);
                        }
                    }
                };
                true
            }
            Event::Select(EntryId::SetKeyboardLayout(ref name)) => {
                if *name != context.settings.keyboard_layout {
                    context.settings.keyboard_layout = name.to_string();
                    // FIXME: the keyboard's height might change, in which case,
                    // we shall notify the root view.
                    *self =
                        Keyboard::new(&mut self.rect, self.state.alternate.is_locked(), context);
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Gesture(GestureEvent::Tap(center))
            | Event::Gesture(GestureEvent::HoldFingerShort(center, ..))
                if self.rect.includes(center) =>
            {
                true
            }
            Event::Gesture(GestureEvent::Swipe { start, .. }) if self.rect.includes(start) => true,
            Event::Device(DeviceEvent::Finger { position, .. }) if self.rect.includes(position) => {
                true
            }
            _ => false,
        }
    }

    fn might_skip(&self, evt: &Event) -> bool {
        !matches!(
            *evt,
            Event::Key(..)
                | Event::Gesture(..)
                | Event::Device(DeviceEvent::Finger { .. })
                | Event::Select(..)
        )
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context), fields(rect = ?rect)))]
    fn render(&self, context: &mut AppContext, rect: Rectangle) {
        let fb = context.device.framebuffer_mut();
        for child in &self.children {
            if *child.rect() == rect {
                return;
            }
        }

        if let Some(region) = rect.intersection(&self.rect) {
            fb.draw_rectangle(&region, KEYBOARD_BG);
        }
    }

    fn render_rect(&self, rect: &Rectangle) -> Rectangle {
        rect.intersection(&self.rect).unwrap_or(self.rect)
    }

    fn resize(
        &mut self,
        mut rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let dpi = context.device.dpi();
        let placed = place_rows(&rect, &self.layout, dpi);
        rect.min.y = placed.top;
        let mut index = 0;

        for (i, row) in self.layout.keys.iter().enumerate() {
            let y = placed.start_y + i as f32 * (placed.padding + placed.key_height);
            let start_x = row_start_x(
                rect.min.x,
                rect.width(),
                placed.padding,
                placed.key_height,
                &self.layout.widths[i],
            );
            let mut dx = 0.0;

            for j in 0..row.len() {
                let key_width = self.layout.widths[i][j] * placed.key_height;
                let x = start_x + dx;
                dx += key_width + placed.padding;
                let key_rect = rect![
                    x.round() as i32,
                    y.round() as i32,
                    (x + key_width).round() as i32,
                    (y + placed.key_height).round() as i32
                ];
                self.children[index].resize(key_rect, hub, rq, context);
                index += 1;
            }
        }

        self.rect = rect;
        context.kb_rect = rect;
    }

    fn is_background(&self) -> bool {
        true
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

lazy_static! {
    // Most of the combination sequences come from X.org.
    // The chosen characters come from the layout described by
    // Robert Bringhurst in *The Elements of Typographic Style*,
    // version 3.1, p. 92.
    pub static ref DEFAULT_COMBINATIONS: FxHashMap<&'static str, char> = {
        let mut m = FxHashMap::default();
        m.insert("oe", 'œ');
        m.insert("Oe", 'Œ');
        m.insert("ae", 'æ');
        m.insert("AE", 'Æ');
        m.insert("c,", 'ç');
        m.insert("C,", 'Ç');
        m.insert("a;", 'ą');
        m.insert("e;", 'ę');
        m.insert("A;", 'Ą');
        m.insert("E;", 'Ę');
        m.insert("a~", 'ã');
        m.insert("o~", 'õ');
        m.insert("n~", 'ñ');
        m.insert("A~", 'Ã');
        m.insert("O~", 'Õ');
        m.insert("N~", 'Ñ');
        m.insert("a'", 'á');
        m.insert("e'", 'é');
        m.insert("i'", 'í');
        m.insert("o'", 'ó');
        m.insert("u'", 'ú');
        m.insert("y'", 'ý');
        m.insert("z'", 'ź');
        m.insert("s'", 'ś');
        m.insert("c'", 'ć');
        m.insert("n'", 'ń');
        m.insert("A'", 'Á');
        m.insert("E'", 'É');
        m.insert("I'", 'Í');
        m.insert("O'", 'Ó');
        m.insert("U'", 'Ú');
        m.insert("Y'", 'Ý');
        m.insert("Z'", 'Ź');
        m.insert("S'", 'Ś');
        m.insert("C'", 'Ć');
        m.insert("N'", 'Ń');
        m.insert("a`", 'à');
        m.insert("e`", 'è');
        m.insert("i`", 'ì');
        m.insert("o`", 'ò');
        m.insert("u`", 'ù');
        m.insert("A`", 'À');
        m.insert("E`", 'È');
        m.insert("I`", 'Ì');
        m.insert("O`", 'Ò');
        m.insert("U`", 'Ù');
        m.insert("a^", 'â');
        m.insert("e^", 'ê');
        m.insert("i^", 'î');
        m.insert("o^", 'ô');
        m.insert("u^", 'û');
        m.insert("w^", 'ŵ');
        m.insert("y^", 'ŷ');
        m.insert("A^", 'Â');
        m.insert("E^", 'Ê');
        m.insert("I^", 'Î');
        m.insert("O^", 'Ô');
        m.insert("U^", 'Û');
        m.insert("W^", 'Ŵ');
        m.insert("Y^", 'Ŷ');
        m.insert("a:", 'ä');
        m.insert("e:", 'ë');
        m.insert("i:", 'ï');
        m.insert("o:", 'ö');
        m.insert("u:", 'ü');
        m.insert("y:", 'ÿ');
        m.insert("A:", 'Ä');
        m.insert("E:", 'Ë');
        m.insert("I:", 'Ï');
        m.insert("O:", 'Ö');
        m.insert("U:", 'Ü');
        m.insert("Y:", 'Ÿ');
        m.insert("u\"", 'ű');
        m.insert("o\"", 'ő');
        m.insert("U\"", 'Ű');
        m.insert("O\"", 'Ő');
        m.insert("z.", 'ż');
        m.insert("Z.", 'Ż');
        m.insert("th", 'þ');
        m.insert("Th", 'Þ');
        m.insert("ao", 'å');
        m.insert("Ao", 'Å');
        m.insert("l/", 'ł');
        m.insert("d/", 'đ');
        m.insert("o/", 'ø');
        m.insert("L/", 'Ł');
        m.insert("D/", 'Đ');
        m.insert("O/", 'Ø');
        m.insert("mu", 'µ');
        m.insert("l-", '£');
        m.insert("pp", '¶');
        m.insert("so", '§');
        m.insert("|-", '†');
        m.insert("|=", '‡');
        m.insert("ss", 'ß');
        m.insert("Ss", 'ẞ');
        m.insert("o_", 'º');
        m.insert("a_", 'ª');
        m.insert("oo", '°');
        m.insert("!!", '¡');
        m.insert("??", '¿');
        m.insert(".-", '·');
        m.insert(".=", '•');
        m.insert(".>", '›');
        m.insert(".<", '‹');
        m.insert("'1", '′');
        m.insert("'2", '″');
        m.insert("[[", '⟦');
        m.insert("]]", '⟧');
        m.insert("+-", '±');
        m.insert("-:", '÷');
        m.insert("<=", '≤');
        m.insert(">=", '≥');
        m.insert("=/", '≠');
        m.insert("-,", '¬');
        m.insert("~~", '≈');
        m.insert("<<", '«');
        m.insert(">>", '»');
        m.insert("12", '½');
        m.insert("13", '⅓');
        m.insert("23", '⅔');
        m.insert("14", '¼');
        m.insert("34", '¾');
        m.insert("15", '⅕');
        m.insert("25", '⅖');
        m.insert("35", '⅗');
        m.insert("45", '⅘');
        m.insert("16", '⅙');
        m.insert("56", '⅚');
        m.insert("18", '⅛');
        m.insert("38", '⅜');
        m.insert("58", '⅝');
        m.insert("78", '⅞');
        m.insert("#f", '♭');
        m.insert("#n", '♮');
        m.insert("#s", '♯');
        m.insert("%o", '‰');
        m.insert("e=", '€');
        m.insert("or", '®');
        m.insert("oc", '©');
        m.insert("op", '℗');
        m.insert("tm", '™');
        m
    };
}

/// What a key on the Slint keyboard does. Character keys insert [`KeyFace::insert`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FaceAction {
    Character,
    DeleteBack,
    DeleteForward,
    Shift,
    Alternate,
    Submit,
    MoveBack,
    MoveForward,
    MoveStart,
    MoveEnd,
    Dismiss,
    Ignore,
}

/// One key, already resolved for the current shift and alternate state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KeyFace {
    pub label: String,
    pub insert: String,
    pub stretch: f32,
    pub engaged: bool,
    pub compact: bool,
    pub holdable: bool,
    pub action: FaceAction,
}

/// The layout named in settings, or the bundled English layout when that
/// name is not loaded yet.
pub(crate) fn selected_layout(context: &AppContext) -> Layout {
    let name = &context.settings.keyboard_layout;
    if let Some(layout) = context.keyboard_layouts.get(name) {
        return layout.clone();
    }
    context
        .keyboard_layouts
        .get("English")
        .cloned()
        .unwrap_or_else(fallback_english)
}

fn fallback_english() -> Layout {
    serde_json::from_str(include_str!("../../../../keyboard-layouts/english.json"))
        .expect("bundled english keyboard layout")
}

/// Accented and special forms of `base`, in sequence order.
///
/// A form is a combination whose first character is `base`. Holding that
/// key offers these instead of the combine key.
pub(crate) fn variants_for(base: char) -> Vec<char> {
    let mut found: Vec<(&str, char)> = DEFAULT_COMBINATIONS
        .iter()
        .filter(|(sequence, _)| {
            let mut chars = sequence.chars();
            chars.next() == Some(base) && chars.next().is_some() && chars.next().is_none()
        })
        .map(|(sequence, result)| (*sequence, *result))
        .collect();
    found.sort_by(|left, right| left.0.cmp(right.0));
    let mut variants = Vec::new();
    for (_, ch) in found {
        if ch != base && !variants.contains(&ch) {
            variants.push(ch);
        }
        if variants.len() == MAX_HOLD_VARIANTS {
            break;
        }
    }
    variants
}

/// The keys to draw for `layout` at the current modifiers.
///
/// Output placeholders read the matching layer. The combine key is omitted:
/// accented characters come from [`variants_for`] on a held key.
pub(crate) fn key_faces(
    layout: &Layout,
    shift: ModifierState,
    alternate: ModifierState,
) -> Vec<Vec<KeyFace>> {
    let mut level = 0;
    if shift.is_engaged() {
        level += 1;
    }
    if alternate.is_engaged() {
        level += 2;
    }

    layout
        .keys
        .iter()
        .enumerate()
        .map(|(row_index, row)| {
            let mut output_index = 0usize;
            row.iter()
                .enumerate()
                .filter_map(|(column, kind)| {
                    let width = layout
                        .widths
                        .get(row_index)
                        .and_then(|widths| widths.get(column))
                        .copied()
                        .unwrap_or(1.0);
                    key_face(
                        FaceQuery {
                            layout,
                            level,
                            row_index,
                            width,
                            shift,
                            alternate,
                        },
                        *kind,
                        &mut output_index,
                    )
                })
                .collect()
        })
        .collect()
}

struct FaceQuery<'a> {
    layout: &'a Layout,
    level: usize,
    row_index: usize,
    width: f32,
    shift: ModifierState,
    alternate: ModifierState,
}

fn wide_label(width: f32, long: &str, short: &str) -> String {
    if width >= WIDE_KEY {
        long.to_owned()
    } else {
        short.to_owned()
    }
}

fn key_face(query: FaceQuery<'_>, kind: KeyKind, output_index: &mut usize) -> Option<KeyFace> {
    let FaceQuery {
        layout,
        level,
        row_index,
        width,
        shift,
        alternate,
    } = query;
    match kind {
        KeyKind::Combine => None,
        KeyKind::Output(ch) if ch != ' ' => {
            let shown = layout
                .outputs
                .get(level)
                .and_then(|rows| rows.get(row_index))
                .and_then(|row| row.get(*output_index))
                .copied()
                .unwrap_or(ch);
            *output_index += 1;
            Some(character_key(shown, width))
        }
        KeyKind::Output(_) => Some(space_key(width)),
        KeyKind::Delete(LinearDir::Backward) => Some(special(
            wide_label(
                width,
                &crate::fl!("keyboard-key-delete"),
                &crate::fl!("keyboard-key-delete-short"),
            ),
            FaceAction::DeleteBack,
            width,
            false,
        )),
        KeyKind::Delete(LinearDir::Forward) => Some(special(
            crate::fl!("keyboard-key-delete-forward"),
            FaceAction::DeleteForward,
            width,
            false,
        )),
        KeyKind::Shift => Some(special(
            wide_label(
                width,
                &crate::fl!("keyboard-key-shift"),
                &crate::fl!("keyboard-key-shift-short"),
            ),
            FaceAction::Shift,
            width,
            shift.is_locked(),
        )),
        KeyKind::Alternate => Some(special(
            crate::fl!("keyboard-key-alternate"),
            FaceAction::Alternate,
            width,
            alternate.is_locked(),
        )),
        KeyKind::Return => Some(special(
            wide_label(
                width,
                &crate::fl!("keyboard-key-return"),
                &crate::fl!("keyboard-key-return-short"),
            ),
            FaceAction::Submit,
            width,
            false,
        )),
        KeyKind::Move(LinearDir::Backward) => Some(special(
            crate::fl!("keyboard-key-move-back"),
            FaceAction::MoveBack,
            width,
            false,
        )),
        KeyKind::Move(LinearDir::Forward) => Some(special(
            crate::fl!("keyboard-key-move-forward"),
            FaceAction::MoveForward,
            width,
            false,
        )),
        KeyKind::ArrowLeft => Some(special(
            crate::fl!("keyboard-key-arrow-left"),
            FaceAction::MoveBack,
            width,
            false,
        )),
        KeyKind::ArrowRight => Some(special(
            crate::fl!("keyboard-key-arrow-right"),
            FaceAction::MoveForward,
            width,
            false,
        )),
        KeyKind::ArrowUp => Some(special(
            crate::fl!("keyboard-key-arrow-up"),
            FaceAction::MoveStart,
            width,
            false,
        )),
        KeyKind::ArrowDown => Some(special(
            crate::fl!("keyboard-key-arrow-down"),
            FaceAction::MoveEnd,
            width,
            false,
        )),
        KeyKind::Escape => Some(special(
            crate::fl!("keyboard-key-escape"),
            FaceAction::Dismiss,
            width,
            false,
        )),
        KeyKind::Tab => Some(special(
            crate::fl!("keyboard-key-tab"),
            FaceAction::Ignore,
            width,
            false,
        )),
        KeyKind::Control => Some(special(
            crate::fl!("keyboard-key-control"),
            FaceAction::Ignore,
            width,
            false,
        )),
    }
}

fn character_key(ch: char, width: f32) -> KeyFace {
    let label = ch.to_string();
    KeyFace {
        insert: label.clone(),
        compact: false,
        holdable: !variants_for(ch).is_empty(),
        label,
        stretch: width,
        engaged: false,
        action: FaceAction::Character,
    }
}

fn space_key(width: f32) -> KeyFace {
    KeyFace {
        label: crate::fl!("keyboard-key-space"),
        insert: " ".to_string(),
        stretch: width,
        engaged: false,
        compact: true,
        holdable: !variants_for(' ').is_empty(),
        action: FaceAction::Character,
    }
}

fn special(label: String, action: FaceAction, width: f32, engaged: bool) -> KeyFace {
    let compact = label.chars().count() > 1;
    KeyFace {
        label,
        insert: String::new(),
        stretch: width,
        engaged,
        compact,
        holdable: false,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_state_off_is_not_engaged() {
        assert!(!ModifierState::Off.is_engaged());
        assert!(!ModifierState::Off.is_once());
        assert!(!ModifierState::Off.is_locked());
    }

    #[test]
    fn modifier_state_once_is_engaged_and_releases() {
        assert!(ModifierState::Once.is_engaged());
        assert!(ModifierState::Once.is_once());
        assert_eq!(ModifierState::Once.release(), ModifierState::Off);
    }

    #[test]
    fn modifier_state_locked_is_engaged_and_stays_engaged() {
        assert!(ModifierState::Locked.is_engaged());
        assert!(ModifierState::Locked.is_locked());
        assert_eq!(ModifierState::Locked.release(), ModifierState::Locked);
    }

    #[test]
    fn modifier_state_cycles_off_once_locked() {
        assert_eq!(ModifierState::Off.cycle(), ModifierState::Once);
        assert_eq!(ModifierState::Once.cycle(), ModifierState::Locked);
        assert_eq!(ModifierState::Locked.cycle(), ModifierState::Off);
    }

    #[test]
    fn state_defaults_to_off() {
        let state = State::default();
        assert_eq!(state.shift, ModifierState::Off);
        assert_eq!(state.alternate, ModifierState::Off);
        assert_eq!(state.control, ModifierState::Off);
        assert!(!state.combine);
    }

    fn layout(name: &str) -> Layout {
        let path = format!("../../../../keyboard-layouts/{name}.json");
        let text = match name {
            "english" => include_str!("../../../../keyboard-layouts/english.json"),
            "russian" => include_str!("../../../../keyboard-layouts/russian.json"),
            "terminal" => include_str!("../../../../keyboard-layouts/terminal.json"),
            _ => panic!("unknown layout {path}"),
        };
        serde_json::from_str(text).unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    #[test]
    fn english_letters_follow_the_layout_and_label_space_and_delete() {
        let rows = key_faces(&layout("english"), ModifierState::Off, ModifierState::Off);
        assert_eq!(rows[0][0].insert, "q");
        assert!(!rows[0][0].holdable, "q has no accented forms");
        let e = rows
            .iter()
            .flatten()
            .find(|key| key.insert == "e")
            .expect("e");
        assert!(e.holdable, "e should offer accented forms");
        assert_eq!(rows[1][0].action, FaceAction::DeleteBack);
        assert_eq!(rows[1][0].label, crate::fl!("keyboard-key-delete-short"));
        assert_eq!(rows[1][1].insert, "a");
        assert_eq!(rows[2][0].label, crate::fl!("keyboard-key-shift"));
        let space = rows
            .iter()
            .flatten()
            .find(|key| key.insert == " ")
            .expect("space");
        assert_eq!(space.label, crate::fl!("keyboard-key-space"));
        assert!(rows.iter().flatten().all(|key| key.label != "CMB"));
    }

    #[test]
    fn english_alternate_layer_is_digits() {
        let rows = key_faces(
            &layout("english"),
            ModifierState::Off,
            ModifierState::Locked,
        );
        assert_eq!(rows[0][0].insert, "1");
        assert!(rows[0][0].holdable, "1 offers fraction forms");
    }

    #[test]
    fn shift_lock_engages_the_shift_key_and_uppercases() {
        let rows = key_faces(
            &layout("english"),
            ModifierState::Locked,
            ModifierState::Off,
        );
        assert_eq!(rows[0][0].insert, "Q");
        assert!(rows[2][0].engaged);
    }

    #[test]
    fn russian_layout_uses_cyrillic_and_a_short_shift_label() {
        let rows = key_faces(&layout("russian"), ModifierState::Off, ModifierState::Off);
        assert_eq!(rows[0][0].insert, "й");
        assert_eq!(rows[2][0].label, crate::fl!("keyboard-key-shift-short"));
        assert_eq!(rows[2][0].action, FaceAction::Shift);
    }

    #[test]
    fn terminal_layout_shows_escape_arrows_and_the_symbol_layer() {
        let rows = key_faces(&layout("terminal"), ModifierState::Off, ModifierState::Off);
        let labels: Vec<&str> = rows
            .iter()
            .flatten()
            .map(|key| key.label.as_str())
            .collect();
        let escape = crate::fl!("keyboard-key-escape");
        let control = crate::fl!("keyboard-key-control");
        let space = crate::fl!("keyboard-key-space");
        assert!(labels.contains(&escape.as_str()));
        assert!(labels.contains(&control.as_str()));
        assert!(labels.contains(&space.as_str()));
        let shifted = key_faces(
            &layout("terminal"),
            ModifierState::Locked,
            ModifierState::Locked,
        );
        assert_eq!(shifted[0][0].insert, "~");
    }

    #[test]
    fn reader_keyboard_on_a_libra_is_three_bars_with_square_keys() {
        let span = reader_keyboard_span(1264, 300, &layout("english"));
        assert!(
            (span.block_height - 487.0).abs() < 0.1,
            "block {}",
            span.block_height
        );
        assert!(
            (100.0..115.0).contains(&span.key_height),
            "key {}",
            span.key_height
        );
        assert!((span.key_gap - span.key_height * PADDING_RATIO).abs() < 0.1);
    }

    #[test]
    fn reader_keyboard_on_a_narrow_screen_drops_one_bar() {
        let span = reader_keyboard_span(600, 300, &layout("english"));
        assert!(
            (span.block_height - 324.0).abs() < 0.1,
            "block {}",
            span.block_height
        );
        assert!(
            (50.0..53.0).contains(&span.key_height),
            "key {}",
            span.key_height
        );
    }

    #[test]
    fn holding_e_offers_the_acute_and_not_the_capital() {
        let variants = variants_for('e');
        assert!(variants.contains(&'é'));
        assert!(!variants.contains(&'É'));
        assert!(variants_for('E').contains(&'É'));
        assert!(variants_for(' ').is_empty());
    }
}
