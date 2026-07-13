use super::{emulator::Emulator, pty::Pty, render::TerminalRenderer};
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::DeviceIdentity as _;
use crate::device::DevicePaths as _;
use crate::framebuffer::{Framebuffer as _, Pixmap, UpdateMode};
use crate::geom::{CornerSpec, Dir, Point, Rectangle};
use crate::runtime::Job;
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::icon::{Icon, load_icon_pixmap};
use crate::view::menu::{Menu, MenuKind};
use crate::view::toggleable_keyboard::{Layout, ToggleableKeyboard};
use crate::view::{
    BORDER_RADIUS_SMALL, Bus, EntryId, EntryKind, Event, Hub, ID_FEEDER, Id, KeyboardEvent,
    RenderData, RenderQueue, SMALL_BAR_HEIGHT, View, ViewId,
};
use anyhow::Result;
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd as _, RawFd};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

const ICON_NAME: &str = "enclosed_menu";

#[inline]
fn scale_font_size(font_size: f32) -> u32 {
    (font_size * 64.0) as u32
}

pub struct Terminal {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    emulator: Arc<StdMutex<Emulator>>,
    pty: Pty,
    renderer: TerminalRenderer,
    /// 1bpp staging surface; the reader job only feeds the emulator, and the
    /// main loop renders into this pixmap before blitting it to the framebuffer.
    pixmap: Pixmap,
    reader_job: Option<Job>,
    keyboard_index: usize,
    font_size_scaled: u32,
}

impl Terminal {
    pub fn new(
        rect: Rectangle,
        font_size: f32,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        tx: &Hub,
    ) -> Result<Terminal> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let dpi = context.device.dpi();
        let install_dir = context.device.install_dir();
        let font_size_scaled = scale_font_size(font_size);

        // Setup menu icon in top right corner
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let border_radius = scale_by_dpi(BORDER_RADIUS_SMALL, dpi) as i32;
        let menu_pixmap = load_icon_pixmap(ICON_NAME, dpi, &install_dir)
            .expect("terminal menu icon should exist in install icons directory");
        let icon_padding = (small_height - menu_pixmap.width.max(menu_pixmap.height) as i32) / 2;
        let width = menu_pixmap.width as i32 + icon_padding;
        let height = menu_pixmap.height as i32 + icon_padding;
        let dx = (small_height - width) / 2;
        let dy = (small_height - height) / 2;
        let icon_rect = rect![
            rect.max.x - dx - width,
            rect.min.y + dy,
            rect.max.x - dx,
            rect.min.y + dy + height
        ];
        let icon = Icon::new(
            ICON_NAME,
            icon_rect,
            Event::ToggleNear(ViewId::TitleMenu, icon_rect),
        )
        .corners(Some(CornerSpec::Uniform(border_radius)));
        children.push(Box::new(icon) as Box<dyn View>);

        // Add toggleable terminal keyboard with Terminal layout (no bottom bar)
        let mut keyboard = ToggleableKeyboard::new(rect, false)
            .with_layout(Layout::Terminal)
            .with_bottom_bar(false);
        let keyboard_height = keyboard.keyboard_height(context);
        keyboard.set_visible(true, tx, rq, context);
        children.push(Box::new(keyboard) as Box<dyn View>);
        let keyboard_index = children.len() - 1;

        // Grid size for the current layout (keyboard visible)
        let available_width = rect.width() as i32;
        let available_height = rect.height() as i32 - keyboard_height;

        let (rows, cols) = TerminalRenderer::calculate_grid_for_font_size(
            available_width,
            available_height,
            font_size_scaled,
            &mut context.fonts,
            dpi,
        );

        // Max grid size (full screen without keyboard); the renderer clamps
        // against the live screen size so it survives keyboard toggles.
        let (max_rows, max_cols) = TerminalRenderer::calculate_grid_for_font_size(
            rect.width() as i32,
            rect.height() as i32,
            font_size_scaled,
            &mut context.fonts,
            dpi,
        );

        let pty = Pty::spawn(Some("/bin/sh"), rows, cols)?;
        let reader_fd = pty.take_reader_fd()?;
        let emulator = Arc::new(StdMutex::new(Emulator::new(rows, cols)));
        let renderer = TerminalRenderer::new_with_font_size(
            &mut context.fonts,
            max_rows,
            max_cols,
            font_size_scaled,
            dpi,
        );

        let reader_job = spawn_reader_job(reader_fd, Arc::clone(&emulator), tx.clone());
        let pixmap = Pixmap::new(rect.width(), rect.height(), 1);

        rq.add(RenderData::new(id, rect, UpdateMode::Full));
        Ok(Terminal {
            id,
            rect,
            children,
            emulator,
            pty,
            renderer,
            pixmap,
            reader_job: Some(reader_job),
            keyboard_index,
            font_size_scaled,
        })
    }

    fn toggle_keyboard(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut AppContext) {
        let dpi = context.device.dpi();
        if let Some(keyboard) = self.children.get_mut(self.keyboard_index)
            && let Some(kb) = keyboard.downcast_mut::<ToggleableKeyboard>()
        {
            kb.toggle(hub, rq, context);

            let keyboard_height = if kb.is_visible() {
                kb.keyboard_height(context)
            } else {
                0
            };

            let available_width = self.rect.width() as i32;
            let available_height = self.rect.height() as i32 - keyboard_height;

            let (rows, cols) = TerminalRenderer::calculate_grid_for_font_size(
                available_width,
                available_height,
                self.font_size_scaled,
                &mut context.fonts,
                dpi,
            );

            if let Err(e) = self.pty.resize(rows, cols) {
                tracing::warn!(error = %e, "terminal PTY resize failed");
            }

            if let Ok(mut emu) = self.emulator.lock() {
                emu.resize(rows, cols);
            }

            // Force a full redraw of every cell against the new grid.
            self.renderer.clear_screen_state();
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
        }
    }

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
                EntryKind::Command("Toggle Keyboard".to_string(), EntryId::ToggleKeyboard),
                EntryKind::Command("Quit".to_string(), EntryId::Quit),
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
                    Ok(Ok(0)) => return,
                    Ok(Ok(n)) => {
                        if let Ok(mut emu) = emulator.lock() {
                            emu.feed(&buf[..n]);
                        }
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(error = %error, "terminal reader error");
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
                let bytes: &[u8] = match ke {
                    KeyboardEvent::Append(c) => {
                        let s = c.to_string();
                        let _ = self.pty.write(s.as_bytes());
                        return true;
                    }
                    KeyboardEvent::Submit => b"\r",
                    KeyboardEvent::Delete { .. } => &[127],
                    KeyboardEvent::Tab => b"\t",
                    KeyboardEvent::Escape => b"\x1b",
                    KeyboardEvent::Arrow(dir) => match dir {
                        Dir::North => b"\x1b[A",
                        Dir::South => b"\x1b[B",
                        Dir::East => b"\x1b[C",
                        Dir::West => b"\x1b[D",
                    },
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
            Event::Select(EntryId::ToggleKeyboard) => {
                self.toggle_keyboard(hub, rq, context);
                true
            }
            Event::Select(EntryId::Quit) => {
                hub.send(Event::Back.into()).ok();
                true
            }
            Event::WakeUp => {
                if let Ok(emu) = self.emulator.lock() {
                    let dirty = self.renderer.render_screen(
                        emu.screen(),
                        &mut self.pixmap,
                        &mut context.fonts,
                    );
                    if let Some(dirty) = dirty {
                        let update_rect = Rectangle::new(
                            Point::new(
                                self.rect.min.x + dirty.min.x,
                                self.rect.min.y + dirty.min.y,
                            ),
                            Point::new(
                                self.rect.min.x + dirty.max.x,
                                self.rect.min.y + dirty.max.y,
                            ),
                        );
                        rq.add(RenderData::no_wait(
                            self.id,
                            update_rect,
                            UpdateMode::FastMono,
                        ));
                    }
                }
                true
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
            Point::new(rect.min.x - self.rect.min.x, rect.min.y - self.rect.min.y),
            Point::new(rect.max.x - self.rect.min.x, rect.max.y - self.rect.min.y),
        );
        if let Some(clipped) = local_rect.intersection(&pixmap_rect) {
            let dest = Point::new(
                clipped.min.x + self.rect.min.x,
                clipped.min.y + self.rect.min.y,
            );
            fb.draw_framed_pixmap(&self.pixmap, &clipped, dest);
        }
    }

    fn render_rect(&self, rect: &Rectangle) -> Rectangle {
        rect.intersection(&self.rect).unwrap_or(self.rect)
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
    }
}
