//! Fast-region refresh for a progress bar or spinner on an e-ink panel.

use std::cell::{Cell, RefCell};

use crate::framebuffer::UpdateMode;
use crate::geom::Rectangle;
use crate::view::{Id, RenderData, RenderQueue};

/// A2 ink that a later full refresh must clean.
pub(in crate::view) struct FastInk {
    bar: Cell<Rectangle>,
    stains: RefCell<Vec<Rectangle>>,
}

impl FastInk {
    pub(in crate::view) fn new() -> Self {
        Self {
            bar: Cell::new(rect![0, 0, 0, 0]),
            stains: RefCell::new(Vec::new()),
        }
    }

    pub(in crate::view) fn set_bar(&self, bar: Rectangle) {
        self.bar.set(bar);
    }

    pub(in crate::view) fn queue_bar(
        &self,
        id: Id,
        dirty: impl IntoIterator<Item = Rectangle>,
        rq: &mut RenderQueue,
    ) {
        let bar = self.bar.get();
        for rect in dirty {
            let (fast, rest) = split_around_bar(rect, bar);
            if let Some(fast) = fast {
                self.note_stain(fast);
                rq.add(RenderData::no_wait(id, fast, UpdateMode::Fast));
            }
            for part in rest {
                rq.add(RenderData::new(id, part, UpdateMode::Gui));
            }
        }
    }

    pub(in crate::view) fn queue_fast(
        &self,
        id: Id,
        dirty: impl IntoIterator<Item = Rectangle>,
        mode: UpdateMode,
        wait: bool,
        rq: &mut RenderQueue,
    ) {
        for rect in dirty {
            self.note_stain(rect);
            if wait {
                rq.add(RenderData::new(id, rect, mode));
            } else {
                rq.add(RenderData::no_wait(id, rect, mode));
            }
        }
    }

    pub(in crate::view) fn queue_with_clean(
        &self,
        id: Id,
        dirty: Vec<Rectangle>,
        rq: &mut RenderQueue,
    ) {
        if dirty.is_empty() {
            return;
        }
        let stains: Vec<Rectangle> = self
            .stains
            .borrow()
            .iter()
            .copied()
            .map(grow_stain)
            .collect();
        for rect in &dirty {
            let (full, rest) = peel(*rect, &stains);
            for part in full {
                rq.add(RenderData::new(id, part, UpdateMode::Full));
            }
            for part in rest {
                rq.add(RenderData::new(id, part, UpdateMode::Gui));
            }
        }
        release_covered_stains(&mut self.stains.borrow_mut(), &dirty);
    }

    fn note_stain(&self, rect: Rectangle) {
        absorb_stain(&mut self.stains.borrow_mut(), rect);
    }
}

pub(in crate::view) fn grow_stain(rect: Rectangle) -> Rectangle {
    const PAD: i32 = 3;
    rect![
        rect.min.x.saturating_sub(PAD).max(0),
        rect.min.y.saturating_sub(PAD).max(0),
        rect.max.x.saturating_add(PAD),
        rect.max.y.saturating_add(PAD),
    ]
}

pub(in crate::view) fn absorb_stain(stains: &mut Vec<Rectangle>, rect: Rectangle) {
    if rect.is_empty() {
        return;
    }
    if let Some(index) = stains
        .iter()
        .position(|stain| stain.overlaps(&rect) || stain.extends(&rect))
    {
        stains[index].absorb(&rect);
        let merged = stains.remove(index);
        absorb_stain(stains, merged);
        return;
    }
    stains.push(rect);
}

pub(in crate::view) fn peel(
    dirty: Rectangle,
    stains: &[Rectangle],
) -> (Vec<Rectangle>, Vec<Rectangle>) {
    let mut gui = vec![dirty];
    let mut full = Vec::new();
    for stain in stains {
        let mut next = Vec::new();
        for piece in gui {
            let (hit, rest) = split_around_bar(piece, *stain);
            if let Some(hit) = hit {
                full.push(hit);
            }
            next.extend(rest);
        }
        gui = next;
    }
    gui.retain(|part| !part.is_empty());
    (full, gui)
}

pub(in crate::view) fn release_covered_stains(stains: &mut Vec<Rectangle>, dirty: &[Rectangle]) {
    let mut kept = Vec::new();
    for stain in stains.drain(..) {
        let mut pieces = vec![stain];
        for rect in dirty {
            let mut next = Vec::new();
            for piece in pieces {
                let (_hit, rest) = split_around_bar(piece, *rect);
                next.extend(rest);
            }
            pieces = next;
        }
        kept.extend(pieces);
    }
    *stains = kept;
}

pub(in crate::view) fn split_around_bar(
    dirty: Rectangle,
    bar: Rectangle,
) -> (Option<Rectangle>, Vec<Rectangle>) {
    let Some(band) = dirty.intersection(&bar).filter(|band| !band.is_empty()) else {
        return (None, vec![dirty]);
    };
    let mut rest = Vec::new();
    if dirty.min.y < band.min.y {
        rest.push(rect![dirty.min.x, dirty.min.y, dirty.max.x, band.min.y]);
    }
    if band.max.y < dirty.max.y {
        rest.push(rect![dirty.min.x, band.max.y, dirty.max.x, dirty.max.y]);
    }
    if dirty.min.x < band.min.x {
        rest.push(rect![dirty.min.x, band.min.y, band.min.x, band.max.y]);
    }
    if band.max.x < dirty.max.x {
        rest.push(rect![band.max.x, band.min.y, dirty.max.x, band.max.y]);
    }
    rest.retain(|part| !part.is_empty());
    (Some(band), rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_update_leaves_the_label_alone() {
        let bar = rect![10, 80, 200, 110];
        let label = rect![10, 20, 180, 60];
        let (fast, rest) = split_around_bar(label, bar);
        assert_eq!(fast, None);
        assert_eq!(rest, vec![label]);

        let covered = rect![0, 0, 220, 140];
        let (fast, rest) = split_around_bar(covered, bar);
        assert_eq!(fast, Some(bar));
        assert!(rest.iter().all(|part| !part.overlaps(&bar)));
        assert!(rest.iter().any(|part| part.overlaps(&label)));
    }

    #[test]
    fn a_removed_bar_is_fully_refreshed() {
        let mut stains = Vec::new();
        absorb_stain(&mut stains, rect![40, 8, 70, 28]);
        absorb_stain(&mut stains, rect![55, 8, 90, 28]);
        assert_eq!(stains, vec![rect![40, 8, 90, 28]]);

        let page = rect![0, 0, 200, 80];
        let (full, gui) = peel(
            page,
            &stains.iter().copied().map(grow_stain).collect::<Vec<_>>(),
        );
        assert!(full.iter().any(|part| part.contains(&rect![40, 8, 90, 28])));
        assert!(gui.iter().all(|part| !part.overlaps(&rect![40, 8, 90, 28])));

        let label = rect![0, 0, 200, 4];
        let (full, gui) = peel(label, &stains);
        assert!(full.is_empty());
        assert_eq!(gui, vec![label]);

        release_covered_stains(&mut stains, &[page]);
        assert!(stains.is_empty());
    }
}
