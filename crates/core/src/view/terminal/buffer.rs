use crate::framebuffer::Pixmap;
use crate::geom::Rectangle;
use std::collections::VecDeque;

const MAX_DIRTY_RECTS: usize = 16;

/// Writer-side handle: owns the back pixmap, can render without locking.
#[derive(Debug)]
pub struct BufferWriter {
    pub back: Pixmap,
    pub dirty_rect: Option<Rectangle>,
}

/// Shared state protected by mutex: only touched during swap.
#[derive(Debug)]
pub struct DoubleBuffer {
    pub front: Pixmap,
    dirty_rects: VecDeque<Rectangle>,
    needs_full_refresh: bool,
}

impl DoubleBuffer {
    pub fn new(width: u32, height: u32) -> (Self, BufferWriter) {
        let shared = Self {
            front: Pixmap::new(width, height, 1),
            dirty_rects: VecDeque::new(),
            needs_full_refresh: false,
        };
        let writer = BufferWriter {
            back: Pixmap::new(width, height, 1),
            dirty_rect: None,
        };
        (shared, writer)
    }

    /// Swap front and back pixmaps. Called by writer thread after rendering.
    /// After swap, copy front to back so subsequent incremental renders work correctly.
    pub fn swap(&mut self, writer: &mut BufferWriter) {
        std::mem::swap(&mut self.front, &mut writer.back);
        if let Some(rect) = writer.dirty_rect.take() {
            if self.dirty_rects.len() >= MAX_DIRTY_RECTS {
                self.dirty_rects.clear();
                self.needs_full_refresh = true;
            } else {
                self.dirty_rects.push_back(rect);
            }
        }
        writer.back.data.copy_from_slice(&self.front.data);
    }

    pub fn drain_dirty_rects(&mut self) -> impl Iterator<Item = Rectangle> + '_ {
        self.dirty_rects.drain(..)
    }

    pub fn take_full_refresh(&mut self) -> bool {
        std::mem::take(&mut self.needs_full_refresh)
    }

    pub fn is_dirty(&self) -> bool {
        self.needs_full_refresh || !self.dirty_rects.is_empty()
    }
}
