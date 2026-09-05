//! Status-LED command priority categories.

/// Priority tier for a status-LED command.
///
/// Higher tiers win over lower while both are active. Within the same tier,
/// the most recently installed command wins.
///
/// Variant declaration order defines ranking via derived [`Ord`]: later
/// variants outrank earlier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LedPriority {
    /// Soft-suspend awake indication (solid on when armed).
    SoftIndicate,
    /// Full inhibit critical-work blink.
    FullInhibit,
}
