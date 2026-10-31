//! Fast-region refresh for a progress bar or spinner on an e-ink panel.

use std::cell::{Cell, RefCell};

use crate::framebuffer::UpdateMode;
use crate::geom::Rectangle;
use crate::view::slint::RefreshPlan;

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

    /// The bar moves with the fast waveform, everything else with the normal
    /// one. One A2 region that also covers the label leaves the text blurry.
    pub(in crate::view) fn plan_bar(&self, rects: Vec<Rectangle>, plan: &mut RefreshPlan) {
        let bar = self.bar.get();
        for rect in rects {
            let (fast, rest) = split_around_bar(rect, bar);
            if let Some(fast) = fast {
                self.note_stain(fast);
                plan.push(vec![fast], UpdateMode::Fast, false);
            }
            plan.push(rest, UpdateMode::Gui, true);
        }
    }

    /// A spinner tick. Every rectangle is A2 ink and does not wait.
    pub(in crate::view) fn plan_fast(&self, rects: Vec<Rectangle>, plan: &mut RefreshPlan) {
        for rect in &rects {
            self.note_stain(*rect);
        }
        plan.push(rects, UpdateMode::Fast, false);
    }

    /// A frame that is not an A2 tick. Where it covers a fast bar, that
    /// piece is a full refresh. A normal update leaves the fast ink on the
    /// panel.
    pub(in crate::view) fn plan_with_clean(&self, rects: Vec<Rectangle>, plan: &mut RefreshPlan) {
        if rects.is_empty() {
            return;
        }
        let stains: Vec<Rectangle> = self
            .stains
            .borrow()
            .iter()
            .copied()
            .map(grow_stain)
            .collect();
        for rect in &rects {
            let (full, rest) = peel(*rect, &stains);
            plan.push(full, UpdateMode::Full, true);
            plan.push(rest, UpdateMode::Gui, true);
        }
        release_covered_stains(&mut self.stains.borrow_mut(), &rects);
    }

    fn note_stain(&self, rect: Rectangle) {
        absorb_stain(&mut self.stains.borrow_mut(), rect);
    }
}

fn grow_stain(rect: Rectangle) -> Rectangle {
    const PAD: i32 = 3;
    rect![
        rect.min.x.saturating_sub(PAD).max(0),
        rect.min.y.saturating_sub(PAD).max(0),
        rect.max.x.saturating_add(PAD),
        rect.max.y.saturating_add(PAD)
    ]
}

fn absorb_stain(stains: &mut Vec<Rectangle>, rect: Rectangle) {
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

fn peel(dirty: Rectangle, stains: &[Rectangle]) -> (Vec<Rectangle>, Vec<Rectangle>) {
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

fn release_covered_stains(stains: &mut Vec<Rectangle>, dirty: &[Rectangle]) {
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

fn split_around_bar(dirty: Rectangle, bar: Rectangle) -> (Option<Rectangle>, Vec<Rectangle>) {
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

    fn regions(plan: &RefreshPlan) -> Vec<(UpdateMode, bool)> {
        plan.regions
            .iter()
            .map(|region| (region.mode, region.wait))
            .collect()
    }

    #[test]
    fn a_bar_update_leaves_the_label_alone() {
        let ink = FastInk::new();
        ink.set_bar(rect![10, 80, 200, 110]);
        let mut plan = RefreshPlan::default();
        ink.plan_bar(vec![rect![10, 20, 180, 60]], &mut plan);
        assert_eq!(regions(&plan), vec![(UpdateMode::Gui, true)]);

        let mut plan = RefreshPlan::default();
        ink.plan_bar(vec![rect![0, 0, 220, 140]], &mut plan);
        assert_eq!(regions(&plan)[0], (UpdateMode::Fast, false));
        assert!(
            regions(&plan)[1..]
                .iter()
                .all(|region| *region == (UpdateMode::Gui, true))
        );
        assert!(
            plan.regions
                .iter()
                .all(|region| !region.rect.overlaps(&rect![10, 80, 200, 110])
                    || region.mode == UpdateMode::Fast)
        );
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
