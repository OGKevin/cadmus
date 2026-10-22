//! Screen size and hairline lengths written into every Slint window.

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::device::{DeviceCapabilities as _, DeviceHardware as _, DeviceIdentity as _};
use crate::framebuffer::Framebuffer as _;
use crate::view::slint::generated::{InkMetrics, InkScreen, MaterialPalette};

/// The factor that turns one logical pixel into physical pixels.
///
/// A window draws in logical pixels (`physical * 300 / dpi`); the panel draws
/// in physical pixels. One factor serves both directions.
pub(in crate::view) fn pixel_scale(dpi: u16) -> f32 {
    f32::from(dpi.max(1)) / crate::unit::BASE_DPI
}

/// Logical framebuffer size (`physical * 300 / dpi`).
pub(in crate::view) fn logical_size(context: &AppContext) -> (f32, f32) {
    let (width, height) = context.device.framebuffer().dims();
    let scale = pixel_scale(context.device.dpi());
    (width as f32 / scale, height as f32 / scale)
}

/// Hairlines, screen size, and the color-panel flag for one window.
pub(crate) fn apply_ink<W>(window: &W, context: &AppContext)
where
    W: ComponentHandle,
    for<'a> InkMetrics<'a>: Global<'a, W>,
    for<'a> InkScreen<'a>: Global<'a, W>,
    for<'a> MaterialPalette<'a>: Global<'a, W>,
{
    align_strokes(window, context.device.dpi());
    let (width, height) = logical_size(context);
    let screen = window.global::<InkScreen>();
    screen.set_window_width(width);
    screen.set_window_height(height);
    window
        .global::<MaterialPalette>()
        .set_chromatic(context.device.color_samples() > 1);
}

/// Whole-pixel hairline, frame, and radius for `dpi`.
pub(in crate::view) fn align_strokes<W>(window: &W, dpi: u16)
where
    W: ComponentHandle,
    for<'a> InkMetrics<'a>: Global<'a, W>,
{
    let metrics = window.global::<InkMetrics>();
    metrics.set_hairline(device_px(2.0, dpi));
    metrics.set_frame(device_px(3.0, dpi));
    metrics.set_radius(device_px(9.0, dpi));
    metrics.set_mark(device_px(8.0, dpi));
    metrics.set_mark_radius(device_px(4.0, dpi));
}

fn device_px(design_px: f32, dpi: u16) -> f32 {
    logical_px(crate::unit::scale_by_dpi(design_px, dpi), dpi)
}

fn logical_px(physical: f32, dpi: u16) -> f32 {
    (physical + 1.0e-3) * crate::unit::BASE_DPI / f32::from(dpi.max(1))
}
