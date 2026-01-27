use super::{
    buffer::DoubleBuffer,
    emulator::Emulator,
    pty::Pty,
    render::TerminalRenderer,
};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::Fonts;
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{CornerSpec, Rectangle, halves, Point};
use crate::unit::scale_by_dpi;
use crate::view::common::locate_by_id;
use crate::view::icon::{Icon, ICONS_PIXMAPS};
use crate::view::keyboard::Keyboard;
use crate::view::menu::{Menu, MenuKind};
use crate::view::{
    Bus, EntryId, EntryKind, Event, Hub, Id, KeyboardEvent, RenderData, RenderQueue, View, ViewId, ID_FEEDER,
    BORDER_RADIUS_SMALL, SMALL_BAR_HEIGHT, BIG_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use anyhow::Result;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const ICON_NAME: &str = "enclosed_menu";

pub struct Terminal {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    double_buffer: Arc<Mutex<DoubleBuffer>>,
    pty: Pty,
    shutdown_flag: Arc<AtomicBool>,
    reader_thread: Option<JoinHandle<()>>,
}

impl Terminal {
    pub fn new(rect: Rectangle, font_size: f32, rq: &mut RenderQueue, context: &mut Context, hub: &Hub) -> Result<Terminal> {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let font_size_scaled = (font_size * 64.0) as u32;
        
        // Setup menu icon in top right corner
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let border_radius = scale_by_dpi(BORDER_RADIUS_SMALL, dpi) as i32;
        let menu_pixmap = &ICONS_PIXMAPS[ICON_NAME];
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
        
        // Add terminal keyboard
        let big_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (_, big_thickness) = halves(thickness);
        
        let saved_layout = context.settings.keyboard_layout.clone();
        context.settings.keyboard_layout = "Terminal".to_string();
        
        let mut kb_rect = rect![
            rect.min.x,
            rect.max.y - (small_height + 3 * big_height) as i32 + big_thickness,
            rect.max.x,
            rect.max.y
        ];
        let keyboard = Keyboard::new(&mut kb_rect, false, context);
        children.push(Box::new(keyboard) as Box<dyn View>);
        
        context.settings.keyboard_layout = saved_layout;
        
        // Calculate terminal grid size based on available screen space
        let available_width = rect.width() as i32;
        let available_height = (kb_rect.min.y - rect.min.y) as i32;
        let pixmap_width = rect.width();
        let pixmap_height = rect.height();

        let (double_buffer, buffer_writer) = DoubleBuffer::new(pixmap_width, pixmap_height);
        let double_buffer = Arc::new(Mutex::new(double_buffer));
        
        let (rows, cols) = TerminalRenderer::calculate_grid_for_font_size(
            available_width,
            available_height,
            font_size_scaled,
            &mut context.fonts
        );
 
        let pty = Pty::spawn(Some("/bin/sh"), rows, cols)?;
        let mut reader = pty.take_reader()?;
        let pty_fd = pty.as_raw_fd();
        let emulator_shared = Arc::new(Mutex::new(Emulator::new(rows, cols)));
        let shutdown_flag = Arc::new(AtomicBool::new(false));

        let hub = hub.clone();
        let buffer_shared = Arc::clone(&double_buffer);
        let emulator_shared = Arc::clone(&emulator_shared);
        let shutdown_shared = Arc::clone(&shutdown_flag);
        let font_size_scaled_for_thread = font_size_scaled;
        let reader_thread = std::thread::spawn(move || {
            // Load fonts in the reader thread
            let mut fonts = match Fonts::load() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to load fonts in terminal thread: {}", e);
                    return;
                }
            };
            
            let mut renderer = TerminalRenderer::new_with_font_size(&mut fonts, rows, cols, font_size_scaled_for_thread);
            let mut buf = [0u8; 4096];
            let mut writer = buffer_writer;
            
            const POLL_TIMEOUT_MS: i32 = 100;
            
            let mut pfd = pty_fd.map(|fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            });
            
            loop {
                if shutdown_shared.load(Ordering::Acquire) {
                    break;
                }
                
                if let Some(ref mut pollfd) = pfd {
                    pollfd.revents = 0;
                    let ret = unsafe { 
                        libc::poll(pollfd as *mut libc::pollfd, 1, POLL_TIMEOUT_MS) 
                    };
                    
                    if ret < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.kind() != std::io::ErrorKind::Interrupted {
                            eprintln!("Terminal poll error: {}", err);
                            break;
                        }
                        continue;
                    }
                    
                    if ret == 0 {
                        continue;
                    }
                    
                    if pollfd.revents & libc::POLLHUP != 0 {
                        break;
                    }
                    
                    if pollfd.revents & libc::POLLIN == 0 {
                        continue;
                    }
                }
                
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Ok(mut emu) = emulator_shared.lock() {
                            emu.feed(&buf[..n]);
                            let screen = emu.screen();
                            let dirty_rect = renderer.render_screen(screen, &mut writer.back, &mut fonts);
                            writer.dirty_rect = dirty_rect;

                            if let Ok(mut double_buf) = buffer_shared.lock() {
                                double_buf.swap(&mut writer);
                            }
                            hub.send(Event::WakeUp).ok();
                        }
                    }
                    Err(e) => {
                        eprintln!("Terminal reader error: {}", e);
                        break;
                    }
                }
            }
        });
        
        rq.add(RenderData::new(id, rect, UpdateMode::Full));
        let terminal = Terminal {
            id,
            rect,
            children,
            double_buffer,
            pty,
            shutdown_flag,
            reader_thread: Some(reader_thread),
        };
        
        Ok(terminal)
    }
    
    fn toggle_title_menu(
        &mut self,
        rect: Rectangle,
        enable: Option<bool>,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        if let Some(index) = locate_by_id(self, ViewId::TitleMenu) {
            if let Some(true) = enable {
                return;
            }
            
            rq.add(RenderData::expose(*self.child(index).rect(), UpdateMode::FastMono));
            self.children.remove(index);
        } else {
            if let Some(false) = enable {
                return;
            }
            
            let entries = vec![
                EntryKind::Command("Quit".to_string(), EntryId::Quit),
            ];
            let menu = Menu::new(rect, ViewId::TitleMenu, MenuKind::Contextual, entries, context);
            rq.add(RenderData::no_wait(menu.id(), *menu.rect(), UpdateMode::FastMono));
            self.children.push(Box::new(menu) as Box<dyn View>);
        }
    }
}

impl View for Terminal {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
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
                    KeyboardEvent::Raw(b) => b,
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
            Event::Select(EntryId::Quit) => {
                hub.send(Event::Back).ok();
                true
            }
            Event::WakeUp => {
                if let Ok(mut buffer) = self.double_buffer.lock() {
                    if buffer.dirty {
                        buffer.dirty = false;
                        if let Some(dirty_rect) = buffer.dirty_rect.take() {
                            let update_rect = Rectangle::new(
                                Point::new(self.rect.min.x + dirty_rect.min.x, self.rect.min.y + dirty_rect.min.y),
                                Point::new(self.rect.min.x + dirty_rect.max.x, self.rect.min.y + dirty_rect.max.y),
                            );
                            rq.add(RenderData::no_wait(self.id, update_rect, UpdateMode::FastMono));
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }
    
    fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, _fonts: &mut Fonts) {
        if let Ok(buffer) = self.double_buffer.lock() {
            let pixmap = &buffer.front;
            let pixmap_rect = rect![0, 0, pixmap.width as i32, pixmap.height as i32];
            let local_rect = Rectangle::new(
                Point::new(rect.min.x - self.rect.min.x, rect.min.y - self.rect.min.y),
                Point::new(rect.max.x - self.rect.min.x, rect.max.y - self.rect.min.y),
            );
            if let Some(clipped) = local_rect.intersection(&pixmap_rect) {
                let dest = Point::new(clipped.min.x + self.rect.min.x, clipped.min.y + self.rect.min.y);
                fb.draw_framed_pixmap(pixmap, &clipped, dest);
            }
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
        self.shutdown_flag.store(true, Ordering::Release);
        if let Some(handle) = self.reader_thread.take() {
            let _ = handle.join();
        }
    }
}
