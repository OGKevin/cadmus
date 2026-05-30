//! Shared utilities for settings sub-editors (e.g. LibraryEditor,
//! RefreshRateByKindEditor).
//!
//! These helpers avoid duplicating the identical dimension calculations and
//! chrome-building code across every sub-editor.

use crate::color::BLACK;
use crate::device::CURRENT_DEVICE;
use crate::geom::{Rectangle, halves};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::{SMALL_BAR_HEIGHT, THICKNESS_MEDIUM, View};

use super::bottom_bar::{BottomBarVariant, SettingsEditorBottomBar};

/// Returns `(bar_height, separator_thickness, separator_top_half, separator_bottom_half)`
/// based on the current device DPI.
pub fn calculate_dimensions() -> (i32, i32, i32, i32) {
    let dpi = CURRENT_DEVICE.dpi;
    let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
    let separator_thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
    let (separator_top_half, separator_bottom_half) = halves(separator_thickness);
    let bar_height = small_height;

    (
        bar_height,
        separator_thickness,
        separator_top_half,
        separator_bottom_half,
    )
}

/// Builds the black separator line just above the bottom bar.
pub fn build_bottom_separator(
    rect: Rectangle,
    bar_height: i32,
    separator_top_half: i32,
    separator_bottom_half: i32,
) -> Box<dyn View> {
    let separator = Filler::new(
        rect![
            rect.min.x,
            rect.max.y - bar_height - separator_top_half,
            rect.max.x,
            rect.max.y - bar_height + separator_bottom_half
        ],
        BLACK,
    );
    Box::new(separator) as Box<dyn View>
}

/// Builds a two-button bottom bar (close + validate).
pub fn build_two_button_bottom_bar(
    rect: Rectangle,
    bar_height: i32,
    separator_bottom_half: i32,
    variant: BottomBarVariant,
) -> Box<dyn View> {
    let bottom_bar_rect = rect![
        rect.min.x,
        rect.max.y - bar_height + separator_bottom_half,
        rect.max.x,
        rect.max.y
    ];

    Box::new(SettingsEditorBottomBar::new(bottom_bar_rect, variant)) as Box<dyn View>
}
