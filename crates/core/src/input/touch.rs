use rustc_hash::FxHashMap;

use crate::geom::Point;

use super::codes::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE, ABS_MT_TOUCH_MAJOR, ABS_MT_TRACKING_ID,
    ABS_PRESSURE, ABS_X, ABS_Y,
};
use super::event::SourceId;

pub(super) const SINGLE_TOUCH_CODES: TouchCodes = TouchCodes {
    pressure: ABS_PRESSURE,
    x: ABS_X,
    y: ABS_Y,
};

pub(super) const MULTI_TOUCH_CODES_A: TouchCodes = TouchCodes {
    pressure: ABS_MT_TOUCH_MAJOR,
    x: ABS_MT_POSITION_X,
    y: ABS_MT_POSITION_Y,
};

pub(super) const MULTI_TOUCH_CODES_B: TouchCodes = TouchCodes {
    pressure: ABS_MT_PRESSURE,
    ..MULTI_TOUCH_CODES_A
};
/// Axis codes per touch protocol: single-touch and multi-touch digitizers
/// report position and pressure through different evdev codes.
#[derive(Debug)]
pub(super) struct TouchCodes {
    pub(super) pressure: u16,
    pub(super) x: u16,
    pub(super) y: u16,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TouchProto {
    Single,
    MultiA,
    /// Pressure won't indicate a finger release.
    MultiB,
    MultiC,
}
#[cfg_attr(feature = "tracing", tracing::instrument)]
pub(super) fn compute_mirror_axes(rotation: i8, mirroring_scheme: (i8, i8)) -> (bool, bool) {
    let (mxy, dir) = mirroring_scheme;
    let mx = (4 + (mxy + dir)) % 4;
    let my = (4 + (mxy - dir)) % 4;
    let mirror_x = mxy == rotation || mx == rotation;
    let mirror_y = mxy == rotation || my == rotation;
    (mirror_x, mirror_y)
}
#[derive(Debug, Default)]
pub(super) struct TouchState {
    pub(super) position: Point,
    pub(super) pressure: i32,
}
/// Touch report state for a single digitizer.
///
/// Kept per device: several evdev devices share the event pipeline and each
/// ends its reports with `EV_SYN` / `SYN_REPORT`, so a shared pipeline would let
/// an unrelated device's report lift in-contact fingers mid-gesture.
#[derive(Debug, Default)]
pub(super) struct TouchPipeline {
    /// Tracking id of the slot currently being filled, or `0` for single-touch.
    pub(super) id: i32,
    /// Partial reports keyed by tracking id, cleared per `SYN_REPORT` for
    /// multi-touch protocols.
    pub(super) packets: FxHashMap<i32, TouchState>,
    /// Fingers currently in contact, keyed by tracking id.
    pub(super) fingers: FxHashMap<i32, Point>,
}
/// Returns the touch pipeline for `source`, creating it on first sight.
///
/// Synthetic events never reach here, so an injected control event can neither
/// read nor reset touch state. Entries for devices whose readers exited are
/// left in place; their ids are never reused, so they are inert.
///
/// Single-touch has no tracking ids, so one implicit slot is pre-created.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(touch), level = tracing::Level::TRACE))]
pub(super) fn touch_pipeline(
    touch: &mut FxHashMap<SourceId, TouchPipeline>,
    source: SourceId,
    proto: TouchProto,
) -> &mut TouchPipeline {
    let pipeline = touch.entry(source).or_default();
    if proto == TouchProto::Single && pipeline.packets.is_empty() {
        pipeline.packets.insert(pipeline.id, TouchState::default());
    }
    pipeline
}
/// Returns `true` when an `EV_ABS` code cannot be consumed by the touch
/// pipeline, i.e. it matches neither the tracking id nor any axis of the
/// active `tc` table. Such events indicate a profile/protocol mismatch — for
/// example a rotated or wrong-proto `tc` table — rather than lost data.
#[cfg_attr(feature = "tracing", tracing::instrument(level = tracing::Level::TRACE))]
pub(super) fn is_unmapped_abs_code(code: u16, tc: &TouchCodes) -> bool {
    code != ABS_MT_TRACKING_ID && code != tc.x && code != tc.y && code != tc.pressure
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `EV_ABS` code outside the active `tc` table is the signature of a
    /// profile/protocol mismatch and must be classified as unmapped.
    #[test]
    fn unmapped_abs_codes_are_classified() {
        assert!(is_unmapped_abs_code(
            ABS_MT_TOUCH_MAJOR,
            &MULTI_TOUCH_CODES_B
        ));
        assert!(!is_unmapped_abs_code(ABS_MT_PRESSURE, &MULTI_TOUCH_CODES_B));
        assert!(!is_unmapped_abs_code(
            ABS_MT_TRACKING_ID,
            &MULTI_TOUCH_CODES_B
        ));
        assert!(is_unmapped_abs_code(ABS_MT_POSITION_X, &SINGLE_TOUCH_CODES));
        assert!(!is_unmapped_abs_code(ABS_X, &SINGLE_TOUCH_CODES));
    }
}
