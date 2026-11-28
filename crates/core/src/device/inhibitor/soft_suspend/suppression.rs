//! Suppression window shared by the soft-suspend backends.
//!
//! Before the device goes to sleep, Cadmus lets go of its wake lock (the
//! thing that keeps the device awake) so the kernel can sleep. The time from
//! "let go" until "take it back after waking" is one window. Every window
//! gets its own number, the [`SuppressionEpoch`]. Numbers are never reused, so
//! a note saying "I paused during window 3" cannot be mistaken for window 4.

use super::SuppressionEpoch;

/// One window, plus the number of the latest window.
///
/// [`Self::begin`] joins the window if one is open, or opens a new one with a
/// new number. [`Self::end`] closes it and moves the number on, so open, close,
/// open again never hands out the same number twice.
#[derive(Debug, Default)]
pub(crate) struct Suppression {
    active: bool,
    epoch: u64,
}

impl Suppression {
    /// Starts or rejoins the window and returns the epoch that identifies it.
    pub(crate) fn begin(&mut self) -> SuppressionEpoch {
        if !self.active {
            self.active = true;
            self.epoch += 1;
        }
        SuppressionEpoch(self.epoch)
    }

    /// Ends the window if active, advancing the epoch so no later window reuses it.
    pub(crate) fn end(&mut self) {
        if self.active {
            self.active = false;
            self.epoch += 1;
        }
    }

    /// Whether the window is currently suppressing wake-lock writes.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    /// Whether `epoch` identifies the live window.
    pub(crate) fn is_current(&self, epoch: SuppressionEpoch) -> bool {
        self.active && self.epoch == epoch.0
    }
}

#[cfg(test)]
mod tests {
    use super::Suppression;

    #[test]
    fn begin_while_active_rejoins_the_window() {
        let mut suppression = Suppression::default();
        let first = suppression.begin();
        assert_eq!(suppression.begin(), first);
        assert!(suppression.is_current(first));
    }

    #[test]
    fn later_begin_gets_a_new_epoch() {
        let mut suppression = Suppression::default();
        let first = suppression.begin();
        suppression.end();
        let second = suppression.begin();
        assert_ne!(first, second);
        assert!(second.0 > first.0);
    }

    #[test]
    fn end_then_begin_never_reuses_an_epoch() {
        let mut suppression = Suppression::default();
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(suppression.begin());
            suppression.end();
        }
        for (index, epoch) in seen.iter().enumerate() {
            assert!(
                seen[..index].iter().all(|earlier| earlier != epoch),
                "epoch {epoch:?} was reused"
            );
        }
    }

    #[test]
    fn stale_epoch_is_not_current() {
        let mut suppression = Suppression::default();
        let first = suppression.begin();
        suppression.end();
        assert!(!suppression.is_current(first));

        let second = suppression.begin();
        assert!(!suppression.is_current(first));
        assert!(suppression.is_current(second));
    }
}
