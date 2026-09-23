use super::{
    emulator::Emulator,
    pty::{Pty, TerminalPty, TerminalSize},
    render::{TerminalGeometry, TerminalRenderer},
};
use crate::color::BLACK;
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::DeviceIdentity as _;
use crate::fl;
use crate::framebuffer::{Framebuffer as _, Pixmap, UpdateMode};
use crate::geom::{Dir, Point, Rectangle, halves};
use crate::gesture::GestureEvent;
use crate::input::FingerStatus;
use crate::runtime::Job;
use crate::unit::scale_by_dpi;
use crate::view::common::{
    locate, locate_by_id, toggle_battery_menu, toggle_clock_menu, toggle_main_menu,
};
use crate::view::filler::Filler;
use crate::view::label::Label;
use crate::view::menu::{Menu, MenuKind};
use crate::view::slider::Slider;
use crate::view::toggleable_keyboard::{Layout, ToggleableKeyboard};
use crate::view::top_bar::{TopBar, TopBarVariant};
use crate::view::{
    Align, Bus, EntryId, EntryKind, Event, Hub, ID_FEEDER, Id, KeyboardEvent, RenderData,
    RenderQueue, SMALL_BAR_HEIGHT, SliderId, THICKNESS_MEDIUM, View, ViewId,
};
use anyhow::Result;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd as _, RawFd};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 24.0;
const TERMINAL_BAR_CHILD_COUNT: usize = 5;
const PAGE_UP: &[u8] = b"\x1b[5~";
const PAGE_DOWN: &[u8] = b"\x1b[6~";

fn cursor_key_sequence(dir: Dir, application_cursor: bool) -> &'static [u8] {
    match (dir, application_cursor) {
        (Dir::North, false) => b"\x1b[A",
        (Dir::South, false) => b"\x1b[B",
        (Dir::East, false) => b"\x1b[C",
        (Dir::West, false) => b"\x1b[D",
        (Dir::North, true) => b"\x1bOA",
        (Dir::South, true) => b"\x1bOB",
        (Dir::East, true) => b"\x1bOC",
        (Dir::West, true) => b"\x1bOD",
    }
}

#[inline]
fn scale_font_size(font_size: f32) -> u32 {
    (font_size * 64.0) as u32
}

fn normalized_font_size(current: f32, candidate: f32) -> Option<f32> {
    if !candidate.is_finite() {
        return None;
    }
    let resized = (candidate.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE) * 2.0).round() / 2.0;
    (resized != current).then_some(resized)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalBarRects {
    top_bar: Rectangle,
    top_separator: Rectangle,
    font_size_label: Rectangle,
    slider: Rectangle,
    bottom_separator: Rectangle,
    overlay: Rectangle,
}

fn terminal_bar_rects(rect: Rectangle, dpi: u16) -> TerminalBarRects {
    let bar_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
    let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
    let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
    let top_bar_bottom = rect.min.y + bar_height - separator_top_half;
    let slider_top = rect.min.y + bar_height + separator_bottom_half;
    let slider_bottom = rect.min.y + 2 * bar_height - separator_top_half;
    let overlay_bottom = rect.min.y + 2 * bar_height + separator_bottom_half;
    let font_size_label_right = (rect.min.x + 2 * bar_height).min(rect.max.x);
    TerminalBarRects {
        top_bar: rect![rect.min.x, rect.min.y, rect.max.x, top_bar_bottom],
        top_separator: rect![rect.min.x, top_bar_bottom, rect.max.x, slider_top],
        font_size_label: rect![rect.min.x, slider_top, font_size_label_right, slider_bottom],
        slider: rect![font_size_label_right, slider_top, rect.max.x, slider_bottom],
        bottom_separator: rect![rect.min.x, slider_bottom, rect.max.x, overlay_bottom],
        overlay: rect![rect.min.x, rect.min.y, rect.max.x, overlay_bottom],
    }
}

fn terminal_bar_children(
    rect: Rectangle,
    font_size: f32,
    context: &mut AppContext,
) -> Vec<Box<dyn View>> {
    let bar_rects = terminal_bar_rects(rect, context.device.dpi());
    vec![
        Box::new(TopBar::new(
            bar_rects.top_bar,
            TopBarVariant::Back,
            super::title(),
            context,
        )),
        Box::new(Filler::new(bar_rects.top_separator, BLACK)),
        Box::new(Label::new(
            bar_rects.font_size_label,
            fl!("terminal-font-size"),
            Align::Center,
        )),
        Box::new(Slider::new(
            bar_rects.slider,
            SliderId::FontSize,
            font_size,
            MIN_FONT_SIZE,
            MAX_FONT_SIZE,
        )),
        Box::new(Filler::new(bar_rects.bottom_separator, BLACK)),
    ]
}

fn should_dismiss_terminal_bar(top_bar_visible: bool, overlay: Rectangle, point: Point) -> bool {
    top_bar_visible && !overlay.includes(point)
}

fn toggles_terminal_bar(gesture: GestureEvent) -> bool {
    matches!(
        gesture,
        GestureEvent::Arrow {
            dir: Dir::South,
            ..
        }
    )
}

fn requests_terminal_hard_refresh(gesture: GestureEvent, rect: Rectangle) -> bool {
    matches!(gesture, GestureEvent::HoldFingerLong(point, _) if rect.includes(point))
}

fn application_scroll_sequence(dir: Dir) -> Option<&'static [u8]> {
    match dir {
        Dir::South => Some(PAGE_UP),
        Dir::North => Some(PAGE_DOWN),
        Dir::West | Dir::East => None,
    }
}

fn scrollback_target(current: usize, visible_rows: u16, dir: Dir) -> Option<usize> {
    let half_view = (usize::from(visible_rows) / 2).max(1);
    match dir {
        Dir::South => Some(current.saturating_add(half_view)),
        Dir::North => Some(current.saturating_sub(half_view)),
        Dir::West | Dir::East => None,
    }
}

fn terminal_pixel_dimensions(rect: Rectangle, keyboard_height: i32) -> (u16, u16) {
    (
        rect.width().min(u16::MAX as u32) as u16,
        (rect.height() as i32 - keyboard_height).clamp(0, u16::MAX as i32) as u16,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RendererConfiguration {
    font_size: u32,
    rows: u16,
    cols: u16,
    pixmap_width: u32,
    pixmap_height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalLayout {
    renderer: RendererConfiguration,
    pixel_width: u16,
    pixel_height: u16,
    char_width: i32,
    char_height: i32,
}

impl TerminalLayout {
    fn cell_at(self, rect: Rectangle, point: Point) -> Option<(u16, u16)> {
        if !rect.includes(point) {
            return None;
        }
        let local = point - rect.min;
        if local.x >= i32::from(self.pixel_width) || local.y >= i32::from(self.pixel_height) {
            return None;
        }
        let col = local.x / self.char_width;
        let row = local.y / self.char_height;
        if col >= i32::from(self.renderer.cols) || row >= i32::from(self.renderer.rows) {
            return None;
        }
        Some((col as u16 + 1, row as u16 + 1))
    }
}

fn append_utf8_mouse_value(output: &mut Vec<u8>, value: u16) -> Option<()> {
    let character = char::from_u32(u32::from(value) + 32)?;
    let mut encoded = [0; 4];
    output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
    Some(())
}

fn legacy_mouse_event(
    button: u16,
    col: u16,
    row: u16,
    encoding: vt100::MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    let mut output = b"\x1b[M".to_vec();
    match encoding {
        vt100::MouseProtocolEncoding::Default => {
            output.extend([
                u8::try_from(button + 32).ok()?,
                u8::try_from(col + 32).ok()?,
                u8::try_from(row + 32).ok()?,
            ]);
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            append_utf8_mouse_value(&mut output, button)?;
            append_utf8_mouse_value(&mut output, col)?;
            append_utf8_mouse_value(&mut output, row)?;
        }
        vt100::MouseProtocolEncoding::Sgr => return None,
    }
    Some(output)
}

fn mouse_tap_sequence(
    mode: vt100::MouseProtocolMode,
    encoding: vt100::MouseProtocolEncoding,
    col: u16,
    row: u16,
) -> Option<Vec<u8>> {
    if mode == vt100::MouseProtocolMode::None {
        return None;
    }
    if encoding == vt100::MouseProtocolEncoding::Sgr {
        let mut output = format!("\x1b[<0;{col};{row}M").into_bytes();
        if mode != vt100::MouseProtocolMode::Press {
            output.extend_from_slice(format!("\x1b[<0;{col};{row}m").as_bytes());
        }
        return Some(output);
    }

    let mut output = legacy_mouse_event(0, col, row, encoding)?;
    if mode != vt100::MouseProtocolMode::Press {
        output.extend(legacy_mouse_event(3, col, row, encoding)?);
    }
    Some(output)
}

/// A terminal session that owns a shell PTY, its emulator state, and its views.
///
/// Input events are forwarded to the child shell while terminal output is
/// rendered asynchronously. Resizing synchronizes the PTY, VT100 screen, and
/// renderer before the new layout is displayed.
pub struct Terminal {
    id: Id,
    rect: Rectangle,
    terminal_rect: Rectangle,
    children: Vec<Box<dyn View>>,
    emulator: Arc<StdMutex<Emulator>>,
    pty: Box<dyn TerminalPty>,
    renderer: TerminalRenderer,
    /// 1bpp staging surface; the reader job only feeds the emulator, and the
    /// main loop renders into this pixmap before blitting it to the framebuffer.
    pixmap: Pixmap,
    reader_job: Option<Job>,
    top_bar_visible: bool,
    font_size: f32,
    layout: TerminalLayout,
}

impl Terminal {
    /// Starts a terminal session within `rect` using the configured font size.
    ///
    /// The terminal starts with its on-screen keyboard visible. Failure to open
    /// the PTY, spawn the shell, or create its reader is returned to the caller.
    pub fn new(
        rect: Rectangle,
        font_size: f32,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        tx: &Hub,
    ) -> Result<Terminal> {
        Self::new_with_dependencies(rect, font_size, rq, context, tx, |size| {
            Pty::spawn(Some("/bin/sh"), size).map(|pty| Box::new(pty) as Box<dyn TerminalPty>)
        })
    }

    /// `pty_factory` exists so tests can substitute a fake PTY backend.
    fn new_with_dependencies<P>(
        rect: Rectangle,
        font_size: f32,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        tx: &Hub,
        pty_factory: P,
    ) -> Result<Terminal>
    where
        P: FnOnce(TerminalSize) -> Result<Box<dyn TerminalPty>>,
    {
        let id = ID_FEEDER.next();
        let dpi = context.device.dpi();
        let font_size_scaled = scale_font_size(font_size);

        let terminal_rect = rect;

        let mut keyboard = ToggleableKeyboard::new(terminal_rect, false)
            .with_layout(Layout::Terminal)
            .with_bottom_bar(false);
        let keyboard_height = keyboard.keyboard_height(context);

        // Grid size for the current layout (keyboard visible)
        let available_width = terminal_rect.width() as i32;
        let available_height = terminal_rect.height() as i32 - keyboard_height;

        let geometry = TerminalRenderer::calculate_geometry_for_font_size(
            available_width,
            available_height,
            font_size_scaled,
            &mut context.fonts,
            dpi,
        );
        let (rows, cols) = (geometry.rows, geometry.cols);

        let (pixel_width, pixel_height) = terminal_pixel_dimensions(terminal_rect, keyboard_height);
        let pty = pty_factory(TerminalSize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        })?;
        let reader_fd = pty.take_reader_fd()?;
        let emulator = Arc::new(StdMutex::new(Emulator::new(rows, cols)));
        let renderer = TerminalRenderer::new_with_font_size(
            &mut context.fonts,
            rows,
            cols,
            font_size_scaled,
            dpi,
        );

        let reader_job = spawn_reader_job(reader_fd, Arc::clone(&emulator), tx.clone());
        // The PTY and reader are up; only now is it safe to publish render
        // state, so a failed construction leaves the queue untouched.
        keyboard.set_visible(true, tx, rq, context);
        let pixmap = Pixmap::new(terminal_rect.width(), terminal_rect.height(), 1);
        let layout = TerminalLayout {
            renderer: RendererConfiguration {
                font_size: font_size_scaled,
                rows,
                cols,
                pixmap_width: terminal_rect.width(),
                pixmap_height: terminal_rect.height(),
            },
            pixel_width,
            pixel_height,
            char_width: geometry.char_width,
            char_height: geometry.char_height,
        };

        let mut children = terminal_bar_children(rect, font_size, context);
        children.push(Box::new(keyboard) as Box<dyn View>);
        rq.add(RenderData::new(id, rect, UpdateMode::Full));
        Ok(Terminal {
            id,
            rect,
            terminal_rect,
            children,
            emulator,
            pty,
            renderer,
            pixmap,
            reader_job: Some(reader_job),
            top_bar_visible: true,
            font_size,
            layout,
        })
    }

    fn layout(
        &self,
        terminal_rect: Rectangle,
        font_size: f32,
        context: &mut AppContext,
    ) -> TerminalLayout {
        let keyboard_height = locate::<ToggleableKeyboard>(self)
            .and_then(|index| self.children[index].downcast_ref::<ToggleableKeyboard>())
            .filter(|keyboard| keyboard.is_visible())
            .map(|keyboard| keyboard.keyboard_height(context))
            .unwrap_or(0);
        let (pixel_width, pixel_height) = terminal_pixel_dimensions(terminal_rect, keyboard_height);
        let font_size = scale_font_size(font_size);
        let TerminalGeometry {
            rows,
            cols,
            char_width,
            char_height,
        } = TerminalRenderer::calculate_geometry_for_font_size(
            i32::from(pixel_width),
            i32::from(pixel_height),
            font_size,
            &mut context.fonts,
            context.device.dpi(),
        );
        TerminalLayout {
            renderer: RendererConfiguration {
                font_size,
                rows,
                cols,
                pixmap_width: terminal_rect.width(),
                pixmap_height: terminal_rect.height(),
            },
            pixel_width,
            pixel_height,
            char_width,
            char_height,
        }
    }

    fn apply_layout(
        &mut self,
        font_size: f32,
        persist_font_size: bool,
        context: &mut AppContext,
    ) -> bool {
        self.apply_layout_for_rect(self.terminal_rect, font_size, persist_font_size, context)
    }

    fn apply_layout_for_rect(
        &mut self,
        terminal_rect: Rectangle,
        font_size: f32,
        persist_font_size: bool,
        context: &mut AppContext,
    ) -> bool {
        let layout = self.layout(terminal_rect, font_size, context);
        if let Err(error) = self.pty.resize(TerminalSize {
            rows: layout.renderer.rows,
            cols: layout.renderer.cols,
            pixel_width: layout.pixel_width,
            pixel_height: layout.pixel_height,
        }) {
            tracing::warn!(
                error = %error,
                rows = layout.renderer.rows,
                cols = layout.renderer.cols,
                pixel_width = layout.pixel_width,
                pixel_height = layout.pixel_height,
                "Failed to resize terminal PTY"
            );
            return false;
        }

        if let Ok(mut emulator) = self.emulator.lock() {
            emulator.resize(layout.renderer.rows, layout.renderer.cols);
        }
        // Rebuild the renderer and pixmap for the new grid, then redraw the
        // emulator screen from scratch so no stale pixels survive the resize.
        self.renderer = TerminalRenderer::new_with_font_size(
            &mut context.fonts,
            layout.renderer.rows,
            layout.renderer.cols,
            layout.renderer.font_size,
            context.device.dpi(),
        );
        self.pixmap = Pixmap::new(
            layout.renderer.pixmap_width,
            layout.renderer.pixmap_height,
            1,
        );
        if let Ok(emu) = self.emulator.lock() {
            self.renderer
                .reconstruct_screen(emu.screen(), &mut self.pixmap, &mut context.fonts);
        }
        self.font_size = font_size;
        self.layout = layout;
        self.terminal_rect = terminal_rect;
        if persist_font_size {
            context.settings.terminal.font_size = font_size;
        }
        true
    }

    fn request_render_reconstruction(&mut self, rq: &mut RenderQueue, context: &mut AppContext) {
        if let Ok(emu) = self.emulator.lock() {
            self.renderer
                .reconstruct_screen(emu.screen(), &mut self.pixmap, &mut context.fonts);
        }
        rq.add(RenderData::no_wait(
            self.id,
            self.terminal_rect,
            UpdateMode::Full,
        ));
    }

    fn send_mouse_tap(&mut self, point: Point) -> bool {
        let Some((col, row)) = self.layout.cell_at(self.terminal_rect, point) else {
            return false;
        };
        let Some((mode, encoding)) = self.emulator.lock().ok().map(|emulator| {
            let screen = emulator.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        }) else {
            return false;
        };
        let Some(sequence) = mouse_tap_sequence(mode, encoding, col, row) else {
            return false;
        };
        if let Err(error) = self.pty.write(&sequence) {
            tracing::warn!(error = %error, col, row, "Failed to send terminal mouse tap");
        }
        true
    }

    fn scroll_half_view(&mut self, dir: Dir, rq: &mut RenderQueue) -> bool {
        let Some(application_sequence) = application_scroll_sequence(dir) else {
            return false;
        };
        let (send_to_application, scrollback_changed) = {
            let Ok(mut emulator) = self.emulator.lock() else {
                return false;
            };
            if emulator.alternate_screen() {
                (true, false)
            } else {
                let Some(target) =
                    scrollback_target(emulator.scrollback(), self.layout.renderer.rows, dir)
                else {
                    return false;
                };
                (false, emulator.set_scrollback(target))
            }
        };

        if send_to_application {
            if let Err(error) = self.pty.write(application_sequence) {
                tracing::warn!(error = %error, dir = ?dir, "Failed to send terminal page key");
            }
        } else if scrollback_changed {
            self.request_full_redraw(rq);
        }
        true
    }

    fn return_to_live_output(&mut self, rq: &mut RenderQueue) {
        let scrollback_changed = self
            .emulator
            .lock()
            .is_ok_and(|mut emulator| emulator.set_scrollback(0));
        if scrollback_changed {
            self.request_full_redraw(rq);
        }
    }

    /// Re-render the whole screen and queue a full terminal refresh.
    fn request_full_redraw(&mut self, rq: &mut RenderQueue) {
        self.renderer.clear_screen_state();
        rq.add(RenderData::no_wait(
            self.id,
            self.terminal_rect,
            UpdateMode::Gui,
        ));
    }

    fn includes_terminal_content(&self, point: Point) -> bool {
        Rectangle::new(
            self.terminal_rect.min,
            self.terminal_rect.min
                + pt!(
                    i32::from(self.layout.pixel_width),
                    i32::from(self.layout.pixel_height)
                ),
        )
        .includes(point)
    }

    fn toggle_keyboard(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut AppContext) {
        if let Some(index) = locate::<ToggleableKeyboard>(self)
            && let Some(keyboard) = self.children.get_mut(index)
            && let Some(kb) = keyboard.downcast_mut::<ToggleableKeyboard>()
        {
            kb.toggle(hub, rq, context);
        }
        self.apply_layout(self.font_size, false, context);
    }

    fn resize_children(
        &mut self,
        rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let keyboard_visible = locate::<ToggleableKeyboard>(self)
            .and_then(|index| self.children[index].downcast_ref::<ToggleableKeyboard>())
            .is_some_and(ToggleableKeyboard::is_visible);

        self.children.retain(|child| !child.is::<Menu>());
        if let Some(index) = locate::<ToggleableKeyboard>(self) {
            self.children.remove(index);
        }
        if let Some(index) = locate::<TopBar>(self) {
            self.children.drain(index..index + TERMINAL_BAR_CHILD_COUNT);
        }

        let structural_children = if self.top_bar_visible {
            let children = terminal_bar_children(rect, self.font_size, context);
            let count = children.len();
            self.children.splice(0..0, children);
            count
        } else {
            0
        };

        let mut keyboard = ToggleableKeyboard::new(rect, false)
            .with_layout(Layout::Terminal)
            .with_bottom_bar(false);
        if keyboard_visible {
            keyboard.set_visible(true, hub, rq, context);
        }
        self.children
            .insert(structural_children, Box::new(keyboard));
    }

    fn set_font_size(&mut self, candidate: f32, rq: &mut RenderQueue, context: &mut AppContext) {
        if let Some(font_size) = normalized_font_size(self.font_size, candidate) {
            self.apply_layout(font_size, true, context);
        }
        if let Some(index) = locate::<Slider>(self)
            && let Some(slider) = self.children[index].downcast_mut::<Slider>()
        {
            slider.update(self.font_size, rq);
        }
    }

    fn reseed(&mut self, rq: &mut RenderQueue, context: &mut AppContext) {
        if let Some(index) = locate::<TopBar>(self)
            && let Some(top_bar) = self.children[index].downcast_mut::<TopBar>()
        {
            top_bar.reseed(rq, context);
        }

        rq.add(RenderData::expose(self.rect, UpdateMode::Gui));
    }

    fn toggle_top_bar(&mut self, rq: &mut RenderQueue, context: &mut AppContext) {
        let top_bar_visible = !self.top_bar_visible;
        self.children.retain(|child| !child.is::<Menu>());
        if top_bar_visible {
            self.children.splice(
                0..0,
                terminal_bar_children(self.rect, self.font_size, context),
            );
        } else if let Some(top_bar_index) = locate::<TopBar>(self) {
            self.children
                .drain(top_bar_index..top_bar_index + TERMINAL_BAR_CHILD_COUNT);
        }
        self.top_bar_visible = top_bar_visible;
        rq.add(RenderData::expose(
            terminal_bar_rects(self.rect, context.device.dpi()).overlay,
            UpdateMode::Gui,
        ));
    }

    /// Handles the title event locally because this menu controls the active
    /// terminal instance; bubbling it would open a parent view's title menu.
    fn toggle_title_menu(
        &mut self,
        rect: Rectangle,
        enable: Option<bool>,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        if let Some(index) = locate_by_id(self, ViewId::TitleMenu) {
            if let Some(true) = enable {
                return;
            }

            rq.add(RenderData::expose(
                *self.child(index).rect(),
                UpdateMode::FastMono,
            ));
            self.children.remove(index);
        } else {
            if let Some(false) = enable {
                return;
            }

            let entries = vec![
                EntryKind::Command(fl!("terminal-toggle-keyboard"), EntryId::ToggleKeyboard),
                EntryKind::Command(fl!("top-menu-quit"), EntryId::Quit),
            ];
            let menu = Menu::new(
                rect,
                ViewId::TitleMenu,
                MenuKind::Contextual,
                entries,
                context,
            );
            rq.add(RenderData::no_wait(
                menu.id(),
                *menu.rect(),
                UpdateMode::FastMono,
            ));
            self.children.push(Box::new(menu) as Box<dyn View>);
        }
    }
}

/// Feeds PTY output into the shared emulator and wakes the main loop, which
/// renders the changed cells; stops when cancelled or at EOF.
fn spawn_reader_job(reader_fd: OwnedFd, emulator: Arc<StdMutex<Emulator>>, tx: Hub) -> Job {
    Job::spawn(move |cancel| async move {
        let fd = match AsyncFd::with_interest(reader_fd, Interest::READABLE) {
            Ok(fd) => fd,
            Err(error) => {
                tracing::warn!(error = %error, "terminal PTY reader setup failed");
                return;
            }
        };

        let mut buf = [0u8; 4096];
        loop {
            let mut guard = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                guard = fd.readable() => match guard {
                    Ok(guard) => guard,
                    Err(error) => {
                        tracing::warn!(error = %error, "terminal PTY wait failed");
                        break;
                    }
                },
            };

            loop {
                match guard.try_io(|inner| read_fd(inner.get_ref().as_raw_fd(), &mut buf)) {
                    Ok(Ok(0)) => {
                        tx.send(Event::WakeUp.into()).ok();
                        return;
                    }
                    Ok(Ok(n)) => {
                        if let Ok(mut emu) = emulator.lock() {
                            emu.feed(&buf[..n]);
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error, "terminal reader error");
                        tx.send(Event::WakeUp.into()).ok();
                        return;
                    }
                    Err(_would_block) => break,
                }
            }

            tx.send(Event::WakeUp.into()).ok();
        }
    })
}

/// Raw read on `fd` that retries `EINTR`.
fn read_fd(fd: RawFd, buf: &mut [u8]) -> std::io::Result<usize> {
    loop {
        // SAFETY: `fd` is owned by the `AsyncFd` guarding this call and stays
        // open; `buf` is a valid slice for the duration of the call.
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                buf.len() as libc::size_t,
            )
        };
        if n >= 0 {
            return Ok(n as usize);
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

#[async_trait::async_trait(?Send)]
impl View for Terminal {
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, hub, _bus, rq, context),
            fields(event = ?evt),
            ret(level = tracing::Level::TRACE)
        )
    )]
    async fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::Keyboard(ke) => {
                self.return_to_live_output(rq);
                let bytes: &[u8] = match ke {
                    KeyboardEvent::Append(c) => {
                        let s = c.to_string();
                        let _ = self.pty.write(s.as_bytes());
                        return true;
                    }
                    KeyboardEvent::Submit => b"\r",
                    KeyboardEvent::Delete { .. } => &[127],
                    KeyboardEvent::Arrow(dir) => {
                        let application_cursor = match self.emulator.lock() {
                            Ok(emulator) => emulator.application_cursor(),
                            Err(error) => {
                                tracing::warn!(error = %error, "Failed to read terminal cursor mode");
                                return true;
                            }
                        };
                        cursor_key_sequence(dir, application_cursor)
                    }
                    KeyboardEvent::Tab => b"\t",
                    KeyboardEvent::Escape => b"\x1b",
                    KeyboardEvent::Control(ch) => {
                        let ctrl_byte = ch.to_ascii_uppercase() as u8 - b'A' + 1;
                        let _ = self.pty.write(&[ctrl_byte]);
                        return true;
                    }
                    _ => return true,
                };
                let _ = self.pty.write(bytes);
                true
            }
            Event::ToggleNear(ViewId::TitleMenu, rect) => {
                self.toggle_title_menu(rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::MainMenu, rect) => {
                toggle_main_menu(self, rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::BatteryMenu, rect) => {
                toggle_battery_menu(self, rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::ClockMenu, rect) => {
                toggle_clock_menu(self, rect, None, rq, context);
                true
            }
            Event::Close(ViewId::TitleMenu) => {
                self.toggle_title_menu(Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Close(ViewId::MainMenu) => {
                toggle_main_menu(self, Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Close(ViewId::BatteryMenu) => {
                toggle_battery_menu(self, Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Close(ViewId::ClockMenu) => {
                toggle_clock_menu(self, Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Select(EntryId::ToggleKeyboard) => {
                self.toggle_keyboard(hub, rq, context);
                true
            }
            Event::Slider(SliderId::FontSize, font_size, status) => {
                if status == FingerStatus::Up {
                    self.set_font_size(font_size, rq, context);
                }
                true
            }
            Event::Select(EntryId::Quit) => {
                hub.send(Event::Back.into()).ok();
                true
            }
            Event::Reseed => {
                self.reseed(rq, context);
                true
            }
            Event::Gesture(gesture) if toggles_terminal_bar(gesture) => {
                self.toggle_top_bar(rq, context);
                true
            }
            Event::Gesture(gesture) if requests_terminal_hard_refresh(gesture, self.rect) => {
                self.request_render_reconstruction(rq, context);
                true
            }
            Event::Gesture(GestureEvent::Swipe { dir, start, end })
                if self.includes_terminal_content(start) && self.includes_terminal_content(end) =>
            {
                self.scroll_half_view(dir, rq)
            }
            Event::Gesture(GestureEvent::Tap(point)) if self.top_bar_visible => {
                if should_dismiss_terminal_bar(
                    self.top_bar_visible,
                    terminal_bar_rects(self.rect, context.device.dpi()).overlay,
                    point,
                ) {
                    self.toggle_top_bar(rq, context);
                }
                true
            }
            Event::Gesture(GestureEvent::Tap(point)) => self.send_mouse_tap(point),
            Event::WakeUp => {
                // Reader job owns only the emulator; render its pending output
                // here, but let the event keep bubbling up the view chain.
                if let Ok(emu) = self.emulator.lock() {
                    let dirty = self.renderer.render_screen(
                        emu.screen(),
                        &mut self.pixmap,
                        &mut context.fonts,
                    );
                    if let Some(dirty) = dirty {
                        let update_rect = Rectangle::new(
                            Point::new(
                                self.terminal_rect.min.x + dirty.min.x,
                                self.terminal_rect.min.y + dirty.min.y,
                            ),
                            Point::new(
                                self.terminal_rect.min.x + dirty.max.x,
                                self.terminal_rect.min.y + dirty.max.y,
                            ),
                        );
                        rq.add(RenderData::no_wait(
                            self.id,
                            update_rect,
                            UpdateMode::FastMono,
                        ));
                    }
                }
                false
            }
            _ => false,
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, context), fields(rect = ?rect))
    )]
    fn render(&self, context: &mut AppContext, rect: Rectangle) {
        let fb = context.device.framebuffer_mut();
        let pixmap_rect = rect![0, 0, self.pixmap.width as i32, self.pixmap.height as i32];
        let local_rect = Rectangle::new(
            Point::new(
                rect.min.x - self.terminal_rect.min.x,
                rect.min.y - self.terminal_rect.min.y,
            ),
            Point::new(
                rect.max.x - self.terminal_rect.min.x,
                rect.max.y - self.terminal_rect.min.y,
            ),
        );
        if let Some(clipped) = local_rect.intersection(&pixmap_rect) {
            let dest = Point::new(
                clipped.min.x + self.terminal_rect.min.x,
                clipped.min.y + self.terminal_rect.min.y,
            );
            fb.draw_framed_pixmap(&self.pixmap, &clipped, dest);
        }
    }

    fn render_rect(&self, rect: &Rectangle) -> Rectangle {
        rect.intersection(&self.terminal_rect)
            .unwrap_or(self.terminal_rect)
    }

    fn resize(
        &mut self,
        rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        self.resize_children(rect, hub, rq, context);
        self.rect = rect;
        self.apply_layout_for_rect(rect, self.font_size, false, context);
        rq.add(RenderData::expose(rect, UpdateMode::Full));
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::Terminal)
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The Job's Drop cancels the reader; it can never block here.
        let _ = self.reader_job.take();
        if let Err(error) = self.pty.shutdown() {
            tracing::warn!(error = %error, "Failed to shut down terminal PTY");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PAGE_DOWN, PAGE_UP, RendererConfiguration, Terminal, TerminalLayout,
        application_scroll_sequence, cursor_key_sequence, mouse_tap_sequence, normalized_font_size,
        requests_terminal_hard_refresh, scrollback_target, should_dismiss_terminal_bar,
        spawn_reader_job, terminal_bar_rects, terminal_pixel_dimensions, toggles_terminal_bar,
    };
    use crate::context::test_helpers::create_test_context;
    use crate::device::AppContext;
    use crate::framebuffer::UpdateMode;
    use crate::geom::{Dir, Point, Rectangle};
    use crate::gesture::GestureEvent;
    use crate::view::terminal::emulator::Emulator;
    use crate::view::terminal::pty::{TerminalPty, TerminalSize};
    use crate::view::{Bus, Event, Hub, HubReceiver, KeyboardEvent, RenderQueue, View};
    use anyhow::{Result, bail};
    use std::io::Write;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    #[derive(Default)]
    struct FakePtyState {
        writes: Vec<Vec<u8>>,
        resizes: Vec<TerminalSize>,
        reader_requests: usize,
        shutdowns: usize,
        drops: usize,
        fail_reader: bool,
        fail_resize: bool,
        peer: Option<UnixStream>,
    }

    struct FakePty {
        state: Arc<StdMutex<FakePtyState>>,
    }

    impl FakePty {
        /// Hands out a readable fd whose peer is dropped immediately, so the
        /// reader job sees an idle fd (and EOF once both ends close).
        fn reader_fd(&self) -> Result<OwnedFd> {
            let (reader, peer) = UnixStream::pair()?;
            self.state.lock().expect("fake PTY state poisoned").peer = Some(peer);
            match unsafe { libc::dup(reader.as_raw_fd()) } {
                fd if fd >= 0 => Ok(unsafe { OwnedFd::from_raw_fd(fd) }),
                _ => bail!("failed to duplicate fake reader fd"),
            }
        }
    }

    impl TerminalPty for FakePty {
        fn take_reader_fd(&self) -> Result<OwnedFd> {
            let mut state = self.state.lock().expect("fake PTY state poisoned");
            state.reader_requests += 1;
            if state.fail_reader {
                bail!("fake reader failure");
            }
            drop(state);
            self.reader_fd()
        }

        fn write(&mut self, data: &[u8]) -> anyhow::Result<usize> {
            self.state
                .lock()
                .expect("fake PTY state poisoned")
                .writes
                .push(data.to_vec());
            Ok(data.len())
        }

        fn resize(&self, size: TerminalSize) -> anyhow::Result<()> {
            let mut state = self.state.lock().expect("fake PTY state poisoned");
            if state.fail_resize {
                bail!("fake resize failure");
            }
            state.resizes.push(size);
            Ok(())
        }

        fn shutdown(&mut self) -> anyhow::Result<()> {
            self.state
                .lock()
                .expect("fake PTY state poisoned")
                .shutdowns += 1;
            Ok(())
        }
    }

    impl Drop for FakePty {
        fn drop(&mut self) {
            self.state.lock().expect("fake PTY state poisoned").drops += 1;
        }
    }

    async fn fake_terminal() -> (
        Terminal,
        Arc<StdMutex<FakePtyState>>,
        AppContext,
        Hub,
        HubReceiver,
        RenderQueue,
    ) {
        let state = Arc::new(StdMutex::new(FakePtyState::default()));
        let factory_state = Arc::clone(&state);
        let mut context = create_test_context().await;
        context.load_keyboard_layouts().await;
        let (hub, receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut render_queue = RenderQueue::new();
        let terminal = Terminal::new_with_dependencies(
            Rectangle::new(Point::new(0, 0), Point::new(600, 800)),
            12.0,
            &mut render_queue,
            &mut context,
            &hub,
            move |_| {
                Ok(Box::new(FakePty {
                    state: factory_state,
                }))
            },
        )
        .expect("failed to construct terminal with fake PTY");
        render_queue.clear();

        (terminal, state, context, hub, receiver, render_queue)
    }

    async fn handle_terminal_event(
        terminal: &mut Terminal,
        event: Event,
        hub: &Hub,
        render_queue: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        terminal
            .handle_event(&event, hub, &mut Bus::new(), render_queue, context)
            .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_construction_is_transactional_when_pty_creation_fails() {
        let mut context = create_test_context().await;
        context.load_keyboard_layouts().await;
        let original_keyboard_rect = context.kb_rect;
        let (hub, _) = tokio::sync::mpsc::unbounded_channel();
        let mut render_queue = RenderQueue::new();

        let result = Terminal::new_with_dependencies(
            Rectangle::new(Point::new(0, 0), Point::new(600, 800)),
            12.0,
            &mut render_queue,
            &mut context,
            &hub,
            |_| bail!("fake PTY creation failure"),
        );

        assert!(result.is_err());
        assert!(render_queue.is_empty());
        assert_eq!(context.kb_rect, original_keyboard_rect);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_construction_is_transactional_when_reader_creation_fails() {
        let state = Arc::new(StdMutex::new(FakePtyState {
            fail_reader: true,
            ..FakePtyState::default()
        }));
        let factory_state = Arc::clone(&state);
        let mut context = create_test_context().await;
        context.load_keyboard_layouts().await;
        let original_keyboard_rect = context.kb_rect;
        let (hub, _) = tokio::sync::mpsc::unbounded_channel();
        let mut render_queue = RenderQueue::new();

        let result = Terminal::new_with_dependencies(
            Rectangle::new(Point::new(0, 0), Point::new(600, 800)),
            12.0,
            &mut render_queue,
            &mut context,
            &hub,
            move |_| {
                Ok(Box::new(FakePty {
                    state: factory_state,
                }))
            },
        );

        assert!(result.is_err());
        assert!(render_queue.is_empty());
        assert_eq!(context.kb_rect, original_keyboard_rect);
        let state = state.lock().expect("fake PTY state poisoned");
        assert_eq!(state.reader_requests, 1);
        assert_eq!(state.drops, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn keyboard_events_are_written_to_the_pty_in_terminal_encoding() {
        let (mut terminal, state, mut context, hub, _, mut render_queue) = fake_terminal().await;

        for event in [
            KeyboardEvent::Append('x'),
            KeyboardEvent::Submit,
            KeyboardEvent::Tab,
            KeyboardEvent::Escape,
            KeyboardEvent::Control('c'),
            KeyboardEvent::Arrow(Dir::North),
        ] {
            assert!(
                handle_terminal_event(
                    &mut terminal,
                    Event::Keyboard(event),
                    &hub,
                    &mut render_queue,
                    &mut context,
                )
                .await
            );
        }
        terminal
            .emulator
            .lock()
            .expect("terminal emulator poisoned")
            .feed(b"\x1b[?1h");
        assert!(
            handle_terminal_event(
                &mut terminal,
                Event::Keyboard(KeyboardEvent::Arrow(Dir::South)),
                &hub,
                &mut render_queue,
                &mut context,
            )
            .await
        );

        assert_eq!(
            state.lock().expect("fake PTY state poisoned").writes,
            vec![
                b"x".to_vec(),
                b"\r".to_vec(),
                b"\t".to_vec(),
                b"\x1b".to_vec(),
                vec![3],
                b"\x1b[A".to_vec(),
                b"\x1bOB".to_vec(),
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resize_keeps_pty_emulator_and_renderer_configuration_in_sync() {
        let (mut terminal, state, mut context, hub, _, mut render_queue) = fake_terminal().await;
        let landscape = Rectangle::new(Point::new(0, 0), Point::new(800, 600));

        terminal.resize(landscape, &hub, &mut render_queue, &mut context);

        let expected_size = TerminalSize {
            rows: terminal.layout.renderer.rows,
            cols: terminal.layout.renderer.cols,
            pixel_width: terminal.layout.pixel_width,
            pixel_height: terminal.layout.pixel_height,
        };
        assert_eq!(
            state
                .lock()
                .expect("fake PTY state poisoned")
                .resizes
                .last(),
            Some(&expected_size)
        );
        assert_eq!(
            terminal
                .emulator
                .lock()
                .expect("terminal emulator poisoned")
                .screen()
                .size(),
            (expected_size.rows, expected_size.cols)
        );
        assert_eq!(
            (terminal.pixmap.width, terminal.pixmap.height),
            (landscape.width(), landscape.height())
        );
        assert_eq!(terminal.rect, landscape);
        assert_eq!(terminal.terminal_rect, landscape);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_pty_resize_preserves_the_active_terminal_layout() {
        let (mut terminal, state, mut context, hub, _, mut render_queue) = fake_terminal().await;
        let previous_layout = terminal.layout;
        let previous_terminal_rect = terminal.terminal_rect;
        let previous_pixmap = (
            terminal.pixmap.width,
            terminal.pixmap.height,
            terminal.pixmap.data().to_vec(),
        );
        state.lock().expect("fake PTY state poisoned").fail_resize = true;

        terminal.resize(
            Rectangle::new(Point::new(0, 0), Point::new(800, 600)),
            &hub,
            &mut render_queue,
            &mut context,
        );

        assert_eq!(terminal.layout, previous_layout);
        assert_eq!(terminal.terminal_rect, previous_terminal_rect);
        assert_eq!(
            terminal
                .emulator
                .lock()
                .expect("terminal emulator poisoned")
                .screen()
                .size(),
            (previous_layout.renderer.rows, previous_layout.renderer.cols)
        );
        assert_eq!(
            (terminal.pixmap.width, terminal.pixmap.height),
            (previous_pixmap.0, previous_pixmap.1)
        );
        assert_eq!(terminal.pixmap.data(), previous_pixmap.2.as_slice());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_hold_requests_a_full_framebuffer_update() {
        let (mut terminal, state, mut context, hub, mut receiver, mut render_queue) =
            fake_terminal().await;

        assert!(
            handle_terminal_event(
                &mut terminal,
                Event::Gesture(GestureEvent::HoldFingerLong(Point::new(300, 400), 0)),
                &hub,
                &mut render_queue,
                &mut context,
            )
            .await
        );

        assert_eq!(
            render_queue
                .get(&(UpdateMode::Full, false))
                .expect("full framebuffer update was not scheduled")
                .as_slice(),
            &[(Some(terminal.id), terminal.terminal_rect)]
        );
        // The fake PTY never produced output, so nothing else wakes the loop.
        assert!(receiver.try_recv().is_err());
        let state = state.lock().expect("fake PTY state poisoned");
        assert_eq!(state.shutdowns, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wake_up_publishes_output_without_capturing_the_event() {
        let (mut terminal, _state, mut context, hub, mut receiver, mut render_queue) =
            fake_terminal().await;
        terminal
            .emulator
            .lock()
            .expect("terminal emulator poisoned")
            .feed(b"hi");
        render_queue.clear();

        assert!(
            !handle_terminal_event(
                &mut terminal,
                Event::WakeUp,
                &hub,
                &mut render_queue,
                &mut context,
            )
            .await
        );

        let updates = render_queue
            .get(&(UpdateMode::FastMono, false))
            .expect("terminal output update was not scheduled");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, Some(terminal.id));
        // The fake PTY never produced output, so nothing else wakes the loop.
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_terminal_shuts_down_the_backend_once() {
        let (terminal, state, _, _, _, _) = fake_terminal().await;

        drop(terminal);

        let state = state.lock().expect("fake PTY state poisoned");
        assert_eq!(state.shutdowns, 1);
        assert_eq!(state.drops, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reader_job_drains_final_output_before_eof_and_wakes_the_main_loop() {
        let (reader, mut peer) = UnixStream::pair().expect("failed to create socket pair");
        peer.write_all(b"done")
            .expect("failed to write test output");
        drop(peer);
        let reader_fd = match unsafe { libc::dup(reader.as_raw_fd()) } {
            fd if fd >= 0 => unsafe { OwnedFd::from_raw_fd(fd) },
            _ => panic!("failed to duplicate reader fd"),
        };
        let emulator = Arc::new(StdMutex::new(Emulator::new(2, 10)));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let job = spawn_reader_job(reader_fd, Arc::clone(&emulator), tx);

        let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("reader job did not wake the main loop")
            .expect("hub channel closed");
        assert!(matches!(message.event, Event::WakeUp));
        let outcome = job.join(Duration::from_secs(1)).await;
        assert!(matches!(outcome, crate::runtime::JobOutcome::Finished(())));
        assert_eq!(
            emulator
                .lock()
                .expect("terminal emulator poisoned")
                .screen()
                .contents(),
            "done"
        );
    }

    #[test]
    fn terminal_bar_is_an_overlay_at_the_top_of_the_terminal() {
        let screen = Rectangle::new(Point::new(0, 0), Point::new(600, 800));
        let bar = terminal_bar_rects(screen, 300);

        assert_eq!(bar.overlay.min, screen.min);
        assert_eq!(bar.overlay.max.x, screen.max.x);
        assert!(bar.overlay.max.y < screen.max.y);
        assert_eq!(bar.top_bar.min, screen.min);
        assert_eq!(bar.slider.min.y, bar.top_separator.max.y);
        assert_eq!(bar.slider.max.y, bar.bottom_separator.min.y);
        assert_eq!(bar.font_size_label.max.x, bar.slider.min.x);
        assert_eq!(bar.font_size_label.min.y, bar.slider.min.y);
        assert_eq!(bar.font_size_label.max.y, bar.slider.max.y);
    }

    #[test]
    fn rotation_recalculates_terminal_pixel_dimensions_around_the_keyboard() {
        let portrait = Rectangle::new(Point::new(0, 0), Point::new(600, 800));
        let landscape = Rectangle::new(Point::new(0, 0), Point::new(800, 600));

        assert_eq!(terminal_pixel_dimensions(portrait, 300), (600, 500));
        assert_eq!(terminal_pixel_dimensions(landscape, 300), (800, 300));
        assert_eq!(terminal_pixel_dimensions(landscape, 0), (800, 600));
    }

    #[test]
    fn taps_away_from_a_visible_bar_dismiss_it() {
        let overlay = Rectangle::new(Point::new(0, 0), Point::new(600, 240));

        assert!(!should_dismiss_terminal_bar(
            true,
            overlay,
            Point::new(300, 120)
        ));
        assert!(should_dismiss_terminal_bar(
            true,
            overlay,
            Point::new(300, 400)
        ));
        assert!(!should_dismiss_terminal_bar(
            false,
            overlay,
            Point::new(300, 400)
        ));
    }

    #[test]
    fn only_a_downward_arrow_toggles_the_terminal_bar() {
        let arrow = |dir| GestureEvent::Arrow {
            dir,
            start: Point::new(100, 100),
            end: Point::new(100, 300),
        };

        assert!(toggles_terminal_bar(arrow(Dir::South)));
        assert!(!toggles_terminal_bar(arrow(Dir::North)));
        assert!(!toggles_terminal_bar(GestureEvent::Diamond(Point::new(
            100, 100
        ))));
    }

    #[test]
    fn long_hold_inside_the_terminal_requests_a_hard_refresh() {
        let terminal = Rectangle::new(Point::new(0, 0), Point::new(600, 800));

        assert!(requests_terminal_hard_refresh(
            GestureEvent::HoldFingerLong(Point::new(300, 400), 0),
            terminal,
        ));
        assert!(!requests_terminal_hard_refresh(
            GestureEvent::HoldFingerShort(Point::new(300, 400), 0),
            terminal,
        ));
        assert!(!requests_terminal_hard_refresh(
            GestureEvent::HoldFingerLong(Point::new(700, 400), 0),
            terminal,
        ));
    }

    #[test]
    fn vertical_swipes_map_to_application_page_keys() {
        assert_eq!(application_scroll_sequence(Dir::South), Some(PAGE_UP));
        assert_eq!(application_scroll_sequence(Dir::North), Some(PAGE_DOWN));
        assert_eq!(application_scroll_sequence(Dir::West), None);
        assert_eq!(application_scroll_sequence(Dir::East), None);
    }

    #[test]
    fn cursor_keys_follow_the_terminal_application_mode() {
        assert_eq!(cursor_key_sequence(Dir::North, false), b"\x1b[A");
        assert_eq!(cursor_key_sequence(Dir::South, false), b"\x1b[B");
        assert_eq!(cursor_key_sequence(Dir::East, false), b"\x1b[C");
        assert_eq!(cursor_key_sequence(Dir::West, false), b"\x1b[D");
        assert_eq!(cursor_key_sequence(Dir::North, true), b"\x1bOA");
        assert_eq!(cursor_key_sequence(Dir::South, true), b"\x1bOB");
        assert_eq!(cursor_key_sequence(Dir::East, true), b"\x1bOC");
        assert_eq!(cursor_key_sequence(Dir::West, true), b"\x1bOD");
    }

    #[test]
    fn vertical_swipes_move_half_of_the_visible_rows() {
        assert_eq!(scrollback_target(4, 20, Dir::South), Some(14));
        assert_eq!(scrollback_target(14, 20, Dir::North), Some(4));
        assert_eq!(scrollback_target(4, 20, Dir::West), None);
        assert_eq!(scrollback_target(4, 20, Dir::East), None);
    }

    #[test]
    fn slider_font_sizes_are_rounded_to_half_points() {
        assert_eq!(normalized_font_size(12.0, 9.74), Some(9.5));
        assert_eq!(normalized_font_size(12.0, 9.76), Some(10.0));
        assert_eq!(normalized_font_size(12.0, 12.2), None);
    }

    #[test]
    fn slider_font_sizes_are_clamped_and_non_finite_values_are_ignored() {
        assert_eq!(normalized_font_size(12.0, 1.0), Some(6.0));
        assert_eq!(normalized_font_size(12.0, 30.0), Some(24.0));
        assert_eq!(normalized_font_size(12.0, f32::NAN), None);
        assert_eq!(normalized_font_size(12.0, f32::INFINITY), None);
    }

    #[test]
    fn mouse_tap_is_not_reported_when_tracking_is_disabled() {
        assert_eq!(
            mouse_tap_sequence(
                vt100::MouseProtocolMode::None,
                vt100::MouseProtocolEncoding::Sgr,
                3,
                4,
            ),
            None
        );
    }

    #[test]
    fn x10_mouse_tap_reports_only_the_press() {
        assert_eq!(
            mouse_tap_sequence(
                vt100::MouseProtocolMode::Press,
                vt100::MouseProtocolEncoding::Default,
                5,
                7,
            ),
            Some(vec![0x1b, b'[', b'M', 32, 37, 39])
        );
    }

    #[test]
    fn vt200_mouse_tap_reports_press_and_release() {
        assert_eq!(
            mouse_tap_sequence(
                vt100::MouseProtocolMode::PressRelease,
                vt100::MouseProtocolEncoding::Default,
                5,
                7,
            ),
            Some(vec![
                0x1b, b'[', b'M', 32, 37, 39, 0x1b, b'[', b'M', 35, 37, 39,
            ])
        );
    }

    #[test]
    fn sgr_mouse_tap_reports_press_and_release() {
        assert_eq!(
            mouse_tap_sequence(
                vt100::MouseProtocolMode::ButtonMotion,
                vt100::MouseProtocolEncoding::Sgr,
                12,
                3,
            ),
            Some(b"\x1b[<0;12;3M\x1b[<0;12;3m".to_vec())
        );
    }

    #[test]
    fn tap_position_is_converted_to_one_based_terminal_coordinates() {
        let layout = TerminalLayout {
            renderer: RendererConfiguration {
                font_size: 768,
                rows: 4,
                cols: 10,
                pixmap_width: 100,
                pixmap_height: 80,
            },
            pixel_width: 100,
            pixel_height: 80,
            char_width: 10,
            char_height: 20,
        };
        let rect = Rectangle::new(Point::new(10, 20), Point::new(110, 100));

        assert_eq!(layout.cell_at(rect, Point::new(35, 61)), Some((3, 3)));
        assert_eq!(layout.cell_at(rect, Point::new(110, 61)), None);
    }
}
