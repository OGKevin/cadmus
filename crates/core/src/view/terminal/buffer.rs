use crate::framebuffer::Pixmap;
use crate::geom::Rectangle;
use std::collections::VecDeque;

const MAX_DIRTY_RECTS: usize = 16;

/// Writer-side handle: owns the back pixmap, can render without locking.
#[derive(Debug)]
pub(super) struct BufferWriter {
    pub(super) back: Pixmap,
    pub(super) dirty_rect: Option<Rectangle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RendererConfiguration {
    pub font_size: u32,
    pub rows: u16,
    pub cols: u16,
    pub pixmap_width: u32,
    pub pixmap_height: u32,
}

/// Shared state protected by mutex: only touched during swap.
#[derive(Debug)]
pub(super) struct DoubleBuffer {
    pub(super) front: Pixmap,
    dirty_rects: VecDeque<Rectangle>,
    needs_full_refresh: bool,
    pending_renderer_configuration: Option<RendererConfiguration>,
}

impl DoubleBuffer {
    pub(super) fn new(width: u32, height: u32) -> (Self, BufferWriter) {
        let shared = Self {
            front: Pixmap::new(width, height, 1),
            dirty_rects: VecDeque::new(),
            needs_full_refresh: false,
            pending_renderer_configuration: None,
        };
        let writer = BufferWriter {
            back: Pixmap::new(width, height, 1),
            dirty_rect: None,
        };
        (shared, writer)
    }

    /// Swap front and back pixmaps. Called by writer thread after rendering.
    /// After swap, copy front to back so subsequent incremental renders work correctly.
    pub(super) fn swap(&mut self, writer: &mut BufferWriter) {
        std::mem::swap(&mut self.front, &mut writer.back);
        if let Some(rect) = writer.dirty_rect.take() {
            if self.dirty_rects.len() >= MAX_DIRTY_RECTS {
                self.dirty_rects.clear();
                self.needs_full_refresh = true;
            } else {
                self.dirty_rects.push_back(rect);
            }
        }
        if writer.back.width == self.front.width
            && writer.back.height == self.front.height
            && writer.back.samples == self.front.samples
        {
            writer.back.data.copy_from_slice(&self.front.data);
        } else {
            writer.back = Pixmap::new(self.front.width, self.front.height, self.front.samples);
        }
    }

    pub(super) fn drain_dirty_rects(&mut self) -> impl Iterator<Item = Rectangle> + '_ {
        self.dirty_rects.drain(..)
    }

    pub(super) fn take_full_refresh(&mut self) -> bool {
        std::mem::take(&mut self.needs_full_refresh)
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.needs_full_refresh || !self.dirty_rects.is_empty()
    }

    pub(super) fn request_full_refresh(&mut self) {
        self.dirty_rects.clear();
        self.needs_full_refresh = true;
    }

    pub(super) fn request_renderer_configuration(&mut self, configuration: RendererConfiguration) {
        self.pending_renderer_configuration = Some(configuration);
    }

    pub(super) fn take_renderer_configuration(&mut self) -> Option<RendererConfiguration> {
        self.pending_renderer_configuration.take()
    }
}

#[cfg(test)]
mod tests {
    use super::{DoubleBuffer, RendererConfiguration};
    use crate::framebuffer::Pixmap;

    #[test]
    fn renderer_configuration_requests_are_coalesced() {
        let (mut buffer, _) = DoubleBuffer::new(10, 10);
        buffer.request_renderer_configuration(RendererConfiguration {
            font_size: 640,
            rows: 10,
            cols: 20,
            pixmap_width: 100,
            pixmap_height: 100,
        });
        let latest = RendererConfiguration {
            font_size: 768,
            rows: 8,
            cols: 16,
            pixmap_width: 80,
            pixmap_height: 120,
        };
        buffer.request_renderer_configuration(latest);

        assert_eq!(buffer.take_renderer_configuration(), Some(latest));
        assert_eq!(buffer.take_renderer_configuration(), None);
    }

    #[test]
    fn swap_recreates_back_pixmap_after_resize() {
        let (mut buffer, mut writer) = DoubleBuffer::new(10, 20);
        writer.back = Pixmap::new(30, 40, 1);

        buffer.swap(&mut writer);

        assert_eq!((buffer.front.width, buffer.front.height), (30, 40));
        assert_eq!((writer.back.width, writer.back.height), (30, 40));
        assert!(writer.back.data.iter().all(|pixel| *pixel == 255));
    }
}
