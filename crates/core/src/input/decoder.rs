use std::mem;
use std::time::Duration;

use rustc_hash::FxHashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::framebuffer::Display;
use crate::settings::ButtonScheme;

use super::codes::{
    ABS_MT_TRACKING_ID, BTN_TOUCH, EV_ABS, EV_KEY, EV_MSC, EV_SYN, GYROSCOPE_ROTATIONS, MSC_RAW,
    MSC_RAW_GSENSOR_LANDSCAPE_LEFT, MSC_RAW_GSENSOR_PORTRAIT_DOWN, SYN_DROPPED, SYN_REPORT,
    VAL_PRESS, VAL_RELEASE, VAL_REPEAT,
};
use super::event::{
    ButtonCode, ButtonStatus, DeviceEvent, EventSource, FingerStatus, InputEvent, SourceId,
    TaggedInputEvent, seconds,
};
use super::keyboard::{KeyEvent, Modifier, Modifiers};
use super::profile::{DeviceProfile, KeyAction};
use super::touch::{
    MULTI_TOUCH_CODES_A, MULTI_TOUCH_CODES_B, SINGLE_TOUCH_CODES, TouchCodes, TouchPipeline,
    TouchProto, TouchState, compute_mirror_axes, is_unmapped_abs_code, touch_pipeline,
};

/// Captures the subset of device properties needed for input event processing.
#[derive(Clone, Copy)]
pub struct DeviceInputInfo {
    pub proto: TouchProto,
    pub mark: u8,
    pub mirroring_scheme: (i8, i8),
    pub swapping_scheme: i8,
    pub startup_rotation: i8,
    pub gyro_rotation_transform: GyroRotationTransform,
}

#[derive(Clone, Copy)]
pub struct GyroRotationTransform(fn(i8) -> i8);

impl GyroRotationTransform {
    pub fn new(f: fn(i8) -> i8) -> Self {
        Self(f)
    }

    pub fn transform(&self, n: i8) -> i8 {
        (self.0)(n)
    }
}

impl Default for GyroRotationTransform {
    fn default() -> Self {
        Self(|n| n)
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(rx, info, screen), fields(proto = ?info.proto, mark = info.mark)))]
pub fn device_events(
    mut rx: UnboundedReceiver<TaggedInputEvent>,
    screen: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) -> UnboundedReceiver<DeviceEvent> {
    let Display { dims, rotation } = screen;
    tracing::trace!(
        rotation,
        screen_dims = ?dims,
        proto = ?info.proto,
        mark = info.mark,
        mirroring_scheme = ?info.mirroring_scheme,
        swapping_scheme = info.swapping_scheme,
        startup_rotation = info.startup_rotation,
        "starting device event pipeline"
    );
    let (ty, ry) = tokio::sync::mpsc::unbounded_channel();
    crate::runtime::spawn(async move {
        parse_device_events(
            &mut rx,
            &ty,
            Display { dims, rotation },
            button_scheme,
            info,
        )
        .await;
    });
    ry
}
#[cfg_attr(feature = "tracing", tracing::instrument(skip(transform)))]
fn gsensor_rotation_from_raw(value: i32, transform: GyroRotationTransform) -> Option<i8> {
    if !(MSC_RAW_GSENSOR_PORTRAIT_DOWN..=MSC_RAW_GSENSOR_LANDSCAPE_LEFT).contains(&value) {
        return None;
    }
    GYROSCOPE_ROTATIONS
        .iter()
        .position(|&v| v == value)
        .map(|i| transform.transform(i as i8))
}
/// How often the decoder heartbeat reports its event counters.
///
/// The heartbeat exists so a log can prove the decoder is alive and counting
/// even when no device event reaches it — the #1138 freeze showed the app loop
/// healthy while the digitizer stayed silent, and without this line that state
/// is indistinguishable from a dead decoder.
const DECODER_HEARTBEAT_EVERY: Duration = Duration::from_secs(60);

/// How long a device may go without a report before [`DeviceEvent::UserActivity`]
/// is emitted again.
const ACTIVITY_REPORT_INTERVAL_SECS: libc::time_t = 60;

/// Returns the decoder heartbeat timer.
///
/// A persistent interval rather than a fresh `sleep` per event: the timer keeps
/// ticking while events stream, so the heartbeat still fires. `Interval::tick`
/// is cancel-safe, so polling it inside `select!` against `rx.recv()` neither
/// loses nor duplicates ticks. The first tick resolves immediately, which emits
/// a zero baseline line when the decoder starts.
#[cfg_attr(feature = "tracing", tracing::instrument)]
fn heartbeat_interval() -> tokio::time::Interval {
    let mut heartbeat = tokio::time::interval(DECODER_HEARTBEAT_EVERY);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat
}

/// Emits [`DeviceEvent::UserActivity`] when the device clock passed the
/// activity interval since the last report, and records the new reference time.
///
/// The difference is taken as an absolute value because `tv_sec` wraps on
/// kernels whose `time_t` cannot grow forever, which would otherwise suppress
/// the heartbeat for the rest of the session.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(ty, time), fields(secs = time.tv_sec)))]
fn report_user_activity(
    last_activity: &mut libc::time_t,
    time: libc::timeval,
    ty: &UnboundedSender<DeviceEvent>,
) {
    if (time.tv_sec - *last_activity).abs() < ACTIVITY_REPORT_INTERVAL_SECS {
        return;
    }
    *last_activity = time.tv_sec;
    ty.send(DeviceEvent::UserActivity).ok();
}

/// Event counters for [`parse_device_events`], emitted on the heartbeat tick.
///
/// Read them as a funnel: a device whose reader forwards bytes shows up in
/// `abs`/`key`, bytes the pipeline cannot place show up in `unmapped_abs`
/// (rotated `tc` table, unexpected pressure axis, report without tracking id),
/// and complete reports show up in `syn`/`touch_packets`. `dropped_syn` counts
/// kernel-side event loss.
#[derive(Debug, Default)]
struct DecoderCounters {
    abs: u64,
    key: u64,
    syn: u64,
    unmapped_abs: u64,
    touch_packets: u64,
    dropped_syn: u64,
}
struct TouchMapping {
    dims: (u32, u32),
    rotation: i8,
    mirror_x: bool,
    mirror_y: bool,
    tc: TouchCodes,
    mirroring_scheme: (i8, i8),
    swapping_scheme: i8,
    startup_rotation: i8,
    dir: i8,
}

impl TouchMapping {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(info), fields(rotation = screen.rotation, proto = ?info.proto)))]
    fn new(screen: Display, info: DeviceInputInfo) -> Self {
        let Display { dims, rotation } = screen;
        let mut tc = match info.proto {
            TouchProto::Single => SINGLE_TOUCH_CODES,
            TouchProto::MultiA => MULTI_TOUCH_CODES_A,
            TouchProto::MultiB => MULTI_TOUCH_CODES_B,
            TouchProto::MultiC => MULTI_TOUCH_CODES_B,
        };
        let (mirror_x, mirror_y) = compute_mirror_axes(rotation, info.mirroring_scheme);
        if rotation % 2 == info.swapping_scheme {
            mem::swap(&mut tc.x, &mut tc.y);
        }
        Self {
            dims,
            rotation,
            mirror_x,
            mirror_y,
            tc,
            mirroring_scheme: info.mirroring_scheme,
            swapping_scheme: info.swapping_scheme,
            startup_rotation: info.startup_rotation,
            dir: info.mirroring_scheme.1,
        }
    }

    fn swapped(&self) -> bool {
        self.rotation % 2 == self.swapping_scheme
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self)))]
    fn apply_rotation(&mut self, next_rotation: i8) {
        if next_rotation != self.rotation {
            let delta = (self.rotation - next_rotation).abs();
            let will_swap_axes = delta % 2 == 1;
            if will_swap_axes {
                mem::swap(&mut self.tc.x, &mut self.tc.y);
                mem::swap(&mut self.dims.0, &mut self.dims.1);
            }
            self.rotation = next_rotation;
            let should_mirror = compute_mirror_axes(self.rotation, self.mirroring_scheme);
            self.mirror_x = should_mirror.0;
            self.mirror_y = should_mirror.1;
            tracing::debug!(
                delta,
                will_swap_axes,
                rotation = self.rotation,
                mirror_x = self.mirror_x,
                mirror_y = self.mirror_y,
                dims_after = ?self.dims,
                tc_x = self.tc.x,
                tc_y = self.tc.y,
                axes_swapped = self.swapped(),
                "rotation applied to input mapping"
            );
        } else {
            tracing::debug!(
                rotation = self.rotation,
                "KEY_ROTATE_DISPLAY matches current input rotation; no remap"
            );
        }
    }
}

/// Touch report state, scoped per digitizer: several devices share this
/// pipeline and every evdev device ends each report with EV_SYN /
/// SYN_REPORT, so shared state would let an unrelated button press lift
/// in-contact fingers mid-gesture. Scoping by device also makes a digitizer
/// replug (new source id, fresh state) work without special cases.
struct TouchDecoders {
    proto: TouchProto,
    pipelines: FxHashMap<SourceId, TouchPipeline>,
}

impl TouchDecoders {
    #[cfg_attr(feature = "tracing", tracing::instrument)]
    fn new(proto: TouchProto) -> Self {
        Self {
            proto,
            pipelines: FxHashMap::default(),
        }
    }

    fn get_or_create(&mut self, source: SourceId) -> &mut TouchPipeline {
        touch_pipeline(&mut self.pipelines, source, self.proto)
    }
}

/// Modifier state for keyboards, tracked per source as a Wayland-compatible
/// `Modifiers` value so it can be forwarded directly to a `wl_keyboard`
/// interface in a future Wayland server. Per source because a keyboard that
/// disconnects with a modifier held never sends the release, and that stale
/// bit must not latch into later events from other keyboards.
#[derive(Default)]
struct Keyboards {
    modifiers: FxHashMap<SourceId, Modifiers>,
}

impl Keyboards {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, ty)))]
    fn apply(&mut self, source: SourceId, evt: &InputEvent, ty: &UnboundedSender<DeviceEvent>) {
        let modifiers = self.modifiers.entry(source).or_default();
        if let Some(modifier) = Modifier::from_evdev_code(evt.code) {
            modifiers.set_depressed(modifier, evt.value != 0);
            tracing::trace!(
                modifier = ?modifier,
                mods_depressed = modifiers.mods_depressed,
                keycode = evt.code,
                value = evt.value,
                "keyboard modifier state change"
            );
        } else if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
            ty.send(DeviceEvent::Button {
                time: seconds(evt.time),
                code: ButtonCode::Keyboard(KeyEvent {
                    keycode: evt.code as u32,
                    modifiers: *modifiers,
                }),
                status: button_status,
            })
            .ok();
        }
    }
}

/// Returns the device identity behind `source`, or `None` for synthetic
/// events: they are `EV_KEY` controls and must not touch per-device state.
fn device_source(source: EventSource) -> Option<SourceId> {
    match source {
        #[cfg(any(target_os = "linux", test))]
        EventSource::Device(source) => Some(source),
        EventSource::Synthetic => None,
    }
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(
        skip(rx, ty, screen, button_scheme, info),
        level = tracing::Level::TRACE,
        fields(
            rotation = screen.rotation,
            dims = ?screen.dims,
            proto = ?info.proto,
            mark = info.mark,
            mirroring_scheme = ?info.mirroring_scheme,
            swapping_scheme = info.swapping_scheme,
            startup_rotation = info.startup_rotation,
        )
    )
)]
async fn parse_device_events(
    rx: &mut UnboundedReceiver<TaggedInputEvent>,
    ty: &UnboundedSender<DeviceEvent>,
    screen: Display,
    button_scheme: ButtonScheme,
    info: DeviceInputInfo,
) {
    let DeviceInputInfo {
        proto,
        mark,
        gyro_rotation_transform,
        ..
    } = info;
    let mut last_activity = -60;
    let mut mapping = TouchMapping::new(screen, info);
    let mut decoders = TouchDecoders::new(proto);
    let mut keyboards = Keyboards::default();

    tracing::trace!(
        rotation = mapping.rotation,
        dims = ?mapping.dims,
        mirror_x = mapping.mirror_x,
        mirror_y = mapping.mirror_y,
        mirroring_scheme = ?mapping.mirroring_scheme,
        swapping_scheme = mapping.swapping_scheme,
        startup_rotation = mapping.startup_rotation,
        proto = ?proto,
        mark,
        tc_x = mapping.tc.x,
        tc_y = mapping.tc.y,
        axes_swapped = mapping.swapped(),
        "parse_device_events started"
    );

    let mut button_scheme = button_scheme;
    let mut counters = DecoderCounters::default();
    let mut heartbeat = heartbeat_interval();

    loop {
        tokio::select! {
            tagged = rx.recv() => {
                let Some(tagged) = tagged else { break };
                let evt = tagged.event;
                #[cfg(feature = "tracing")]
                let _span = tracing::trace_span!("processing input event", event = ?evt).entered();

        let device = device_source(tagged.source);

        if evt.kind == EV_ABS {
            counters.abs += 1;
            if is_unmapped_abs_code(evt.code, &mapping.tc) {
                counters.unmapped_abs += 1;
            }
            handle_touch_axis(&mut decoders, &mapping, device, &evt, mark);
        } else if evt.kind == EV_SYN {
            handle_syn_report(
                &mut decoders,
                &mapping,
                device,
                &evt,
                &mut counters,
                &mut last_activity,
                ty,
            );
        } else if evt.kind == EV_KEY {
            counters.key += 1;
            handle_key(
                &evt,
                tagged.profile,
                device,
                &mut keyboards,
                &mut mapping,
                &mut button_scheme,
                ty,
            );
        } else if evt.kind == EV_MSC && evt.code == MSC_RAW {
            handle_gyro(&evt, &mapping, gyro_rotation_transform, ty);
        }
            }
            _ = heartbeat.tick() => {
                tracing::trace!(
                    abs = counters.abs,
                    key = counters.key,
                    syn = counters.syn,
                    unmapped_abs = counters.unmapped_abs,
                    touch_packets = counters.touch_packets,
                    dropped_syn = counters.dropped_syn,
                    sources = decoders.pipelines.len(),
                    tc_x = mapping.tc.x,
                    tc_y = mapping.tc.y,
                    "input decoder heartbeat"
                );
            }
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(decoders, mapping), level = tracing::Level::TRACE))]
fn handle_touch_axis(
    decoders: &mut TouchDecoders,
    mapping: &TouchMapping,
    device: Option<SourceId>,
    evt: &InputEvent,
    mark: u8,
) {
    let proto = decoders.proto;
    let Some(source) = device else {
        return;
    };
    let pipeline = decoders.get_or_create(source);
    if evt.code == ABS_MT_TRACKING_ID {
        if evt.value >= 0 {
            pipeline.id = evt.value;
            pipeline.packets.insert(pipeline.id, TouchState::default());
        }
    } else if evt.code == mapping.tc.x {
        if let Some(state) = pipeline.packets.get_mut(&pipeline.id) {
            state.position.x = if mapping.mirror_x {
                mapping.dims.0 as i32 - 1 - evt.value
            } else {
                evt.value
            };
            tracing::trace!(
                raw = evt.value,
                evt_code = evt.code,
                tc_x = mapping.tc.x,
                tc_y = mapping.tc.y,
                axis = "screen_x",
                mirrored = mapping.mirror_x,
                dim_used = mapping.dims.0,
                result = state.position.x,
                axes_swapped = mapping.swapped(),
                "touch axis raw"
            );
        }
    } else if evt.code == mapping.tc.y {
        if let Some(state) = pipeline.packets.get_mut(&pipeline.id) {
            state.position.y = if mapping.mirror_y {
                mapping.dims.1 as i32 - 1 - evt.value
            } else {
                evt.value
            };
            tracing::trace!(
                raw = evt.value,
                evt_code = evt.code,
                tc_x = mapping.tc.x,
                tc_y = mapping.tc.y,
                axis = "screen_y",
                mirrored = mapping.mirror_y,
                dim_used = mapping.dims.1,
                result = state.position.y,
                axes_swapped = mapping.swapped(),
                "touch axis raw"
            );
        }
    } else if evt.code == mapping.tc.pressure
        && let Some(state) = pipeline.packets.get_mut(&pipeline.id)
    {
        state.pressure = evt.value;
        if proto == TouchProto::Single && mark == 3 && state.pressure == 0 {
            state.position.x = mapping.dims.0 as i32 - 1 - state.position.x;
            mem::swap(&mut state.position.x, &mut state.position.y);
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(decoders, mapping, counters, ty), level = tracing::Level::TRACE))]
fn handle_syn_report(
    decoders: &mut TouchDecoders,
    mapping: &TouchMapping,
    device: Option<SourceId>,
    evt: &InputEvent,
    counters: &mut DecoderCounters,
    last_activity: &mut libc::time_t,
    ty: &UnboundedSender<DeviceEvent>,
) {
    let proto = decoders.proto;
    if evt.code == SYN_REPORT {
        counters.syn += 1;
        report_user_activity(last_activity, evt.time, ty);

        let Some(source) = device else {
            return;
        };
        counters.touch_packets += 1;
        let pipeline = decoders.get_or_create(source);
        if proto == TouchProto::MultiB {
            pipeline.fingers.retain(|other_id, other_position| {
                pipeline.packets.contains_key(other_id)
                    || ty
                        .send(DeviceEvent::Finger {
                            id: *other_id,
                            time: seconds(evt.time),
                            status: FingerStatus::Up,
                            position: *other_position,
                        })
                        .is_err()
            });
        }

        for (&id, state) in &pipeline.packets {
            if state.pressure > 0 {
                tracing::debug!(
                    id,
                    raw_packet = ?state,
                    final_position = ?state.position,
                    tc_x = mapping.tc.x,
                    tc_y = mapping.tc.y,
                    axes_swapped = mapping.swapped(),
                    rotation = mapping.rotation,
                    mirror_x = mapping.mirror_x,
                    mirror_y = mapping.mirror_y,
                    dims = ?mapping.dims,
                    "touch packet"
                );
            }
            if let Some(&pos) = pipeline.fingers.get(&id) {
                if state.pressure > 0 {
                    if state.position != pos {
                        ty.send(DeviceEvent::Finger {
                            id,
                            time: seconds(evt.time),
                            status: FingerStatus::Motion,
                            position: state.position,
                        })
                        .unwrap();
                        pipeline.fingers.insert(id, state.position);
                    }
                } else {
                    ty.send(DeviceEvent::Finger {
                        id,
                        time: seconds(evt.time),
                        status: FingerStatus::Up,
                        position: state.position,
                    })
                    .unwrap();
                    pipeline.fingers.remove(&id);
                }
            } else if state.pressure > 0 {
                tracing::debug!(
                    id,
                    position = ?state.position,
                    pressure = state.pressure,
                    rotation = mapping.rotation,
                    mirror_x = mapping.mirror_x,
                    mirror_y = mapping.mirror_y,
                    dims = ?mapping.dims,
                    "finger down"
                );
                ty.send(DeviceEvent::Finger {
                    id,
                    time: seconds(evt.time),
                    status: FingerStatus::Down,
                    position: state.position,
                })
                .unwrap();
                pipeline.fingers.insert(id, state.position);
            }
        }

        if proto != TouchProto::Single {
            pipeline.packets.clear();
        }
    } else if evt.code == SYN_DROPPED {
        counters.dropped_syn += 1;
        tracing::warn!(
            source = ?device,
            dropped = counters.dropped_syn,
            "kernel dropped input events"
        );
        if let Some(source) = device
            && let Some(pipeline) = decoders.pipelines.remove(&source)
        {
            for (id, position) in pipeline.fingers {
                ty.send(DeviceEvent::Finger {
                    id,
                    time: seconds(evt.time),
                    status: FingerStatus::Up,
                    position,
                })
                .ok();
            }
        }
    }
}

/// Dispatches one `EV_KEY` event to its handler.
///
/// `BTN_TOUCH` is skipped: it is the touch-contact signal from a touch/pen
/// device, not a button.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(keyboards, mapping, ty), level = tracing::Level::TRACE))]
fn handle_key(
    evt: &InputEvent,
    profile: DeviceProfile,
    device: Option<SourceId>,
    keyboards: &mut Keyboards,
    mapping: &mut TouchMapping,
    button_scheme: &mut ButtonScheme,
    ty: &UnboundedSender<DeviceEvent>,
) {
    if evt.code == BTN_TOUCH {
        return;
    }
    match profile.lookup(evt.code) {
        Some(
            action @ (KeyAction::SleepCover | KeyAction::ButtonScheme | KeyAction::RotateDisplay),
        ) => {
            handle_control_action(action, evt, mapping, button_scheme, ty);
        }
        Some(action) => {
            handle_mapped_key(action, evt, mapping, *button_scheme, ty);
        }
        None => {
            if profile.tracks_modifiers
                && let Some(source) = device
            {
                handle_tracked_key(keyboards, source, evt, ty);
            } else if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
                send_button(
                    ty,
                    seconds(evt.time),
                    ButtonCode::Raw(evt.code),
                    button_status,
                    evt,
                );
            }
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(mapping, ty)))]
fn handle_control_action(
    action: KeyAction,
    evt: &InputEvent,
    mapping: &mut TouchMapping,
    button_scheme: &mut ButtonScheme,
    ty: &UnboundedSender<DeviceEvent>,
) {
    match action {
        KeyAction::SleepCover => {
            if evt.value == VAL_PRESS || evt.value == VAL_REPEAT {
                ty.send(DeviceEvent::CoverOn).ok();
            } else if evt.value == VAL_RELEASE {
                ty.send(DeviceEvent::CoverOff).ok();
            }
        }
        KeyAction::ButtonScheme => {
            *button_scheme = if evt.value == VAL_PRESS {
                ButtonScheme::Inverted
            } else {
                ButtonScheme::Natural
            };
        }
        KeyAction::RotateDisplay => {
            let next_rotation = evt.value as i8;
            tracing::debug!(
                from_rotation = mapping.rotation,
                to_rotation = next_rotation,
                dims_before = ?mapping.dims,
                mirror_x_before = mapping.mirror_x,
                mirror_y_before = mapping.mirror_y,
                tc_x = mapping.tc.x,
                tc_y = mapping.tc.y,
                "KEY_ROTATE_DISPLAY received"
            );
            mapping.apply_rotation(next_rotation);
        }
        KeyAction::Power
        | KeyAction::Home
        | KeyAction::Light
        | KeyAction::PageTurnScheme(_)
        | KeyAction::Erase
        | KeyAction::Highlight => {}
        #[cfg(any(target_os = "linux", test))]
        KeyAction::PageTurnRaw(_) => {}
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(mapping, ty)))]
fn handle_mapped_key(
    action: KeyAction,
    evt: &InputEvent,
    mapping: &TouchMapping,
    button_scheme: ButtonScheme,
    ty: &UnboundedSender<DeviceEvent>,
) {
    if let Some(button_status) = ButtonStatus::try_from_raw(evt.value) {
        let time = seconds(evt.time);
        let code = action.to_button_code(
            mapping.rotation,
            button_scheme,
            mapping.startup_rotation,
            mapping.dir,
        );
        send_button(ty, time, code, button_status, evt);
    }
}

fn handle_tracked_key(
    keyboards: &mut Keyboards,
    source: SourceId,
    evt: &InputEvent,
    ty: &UnboundedSender<DeviceEvent>,
) {
    keyboards.apply(source, evt, ty);
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(ty)))]
fn send_button(
    ty: &UnboundedSender<DeviceEvent>,
    time: f64,
    code: ButtonCode,
    status: ButtonStatus,
    evt: &InputEvent,
) {
    tracing::debug!(
        code = ?code,
        status = ?status,
        time,
        raw_code = evt.code,
        raw_value = evt.value,
        "decoded button event"
    );
    ty.send(DeviceEvent::Button { time, code, status }).ok();
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip(mapping, transform, ty)))]
fn handle_gyro(
    evt: &InputEvent,
    mapping: &TouchMapping,
    transform: GyroRotationTransform,
    ty: &UnboundedSender<DeviceEvent>,
) {
    if let Some(next_rotation) = gsensor_rotation_from_raw(evt.value, transform) {
        tracing::trace!(
            raw_value = evt.value,
            next_rotation,
            current_rotation = mapping.rotation,
            "gyroscope rotation event"
        );
        ty.send(DeviceEvent::RotateScreen(next_rotation)).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::input::profile::DeviceProfile;

    use crate::geom::Point;

    use crate::input::codes::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE, KEY_LEFTSHIFT, KEY_POWER,
        KEY_ROTATE_DISPLAY, KEY_SPACE,
    };
    use crate::input::event::InputEvent;

    /// Regression test for [GitHub issue #1089](https://github.com/OGKevin/cadmus/issues/1089):
    /// a quarter-turn rotation must swap the cached input dimensions so
    /// mirrored touch positions use the rotated screen bounds. The regression
    /// was introduced in commit `9b91d69a4`.
    #[tokio::test]
    async fn touch_mapping_swaps_dims_when_rotating_to_landscape() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (2, 1),
            swapping_scheme: 1,
            startup_rotation: 1,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        let screen = Display {
            dims: (1264, 1680),
            rotation: 1,
        };
        crate::runtime::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(&mut rx, &ty, screen, ButtonScheme::Natural, info).await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(SourceId(0)),
        };
        for evt in [
            tagged(EV_KEY, KEY_ROTATE_DISPLAY, 2),
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(EV_ABS, ABS_MT_POSITION_X, 59),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 1178),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }

        let finger = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    position,
                    ..
                }) = ry.recv().await
                {
                    return position;
                }
            }
        })
        .await
        .expect("timed out waiting for touch position");

        assert_eq!(finger, Point { x: 1620, y: 85 });
    }
    /// Receives the next device event, skipping the `UserActivity` heartbeat
    /// triggered by the first `SYN_REPORT` after a 60 s idle window.
    async fn next_input_event(
        ry: &mut tokio::sync::mpsc::UnboundedReceiver<DeviceEvent>,
    ) -> DeviceEvent {
        loop {
            match ry.recv().await {
                Some(DeviceEvent::UserActivity) => continue,
                Some(event) => return event,
                None => panic!("input event channel closed"),
            }
        }
    }
    /// Regression: a `SYN_REPORT` from a non-touch device (e.g. a hardware
    /// button) must not lift fingers still in contact on the digitizer.
    #[tokio::test]
    async fn syn_report_from_other_source_does_not_lift_fingers() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        crate::runtime::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |source, kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(source),
        };
        let touch = SourceId(0);
        let power_button = SourceId(1);
        for evt in [
            tagged(touch, EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(touch, EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(touch, EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(touch, EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(touch, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    ..
                }
            ),
            "finger must be down after the digitizer's report"
        );

        for evt in [
            tagged(power_button, EV_KEY, KEY_POWER, VAL_PRESS),
            tagged(power_button, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(next_input_event(&mut ry).await, DeviceEvent::Button { .. }),
            "the power press must arrive as a button event"
        );
        assert!(
            ry.try_recv().is_err(),
            "a foreign SYN_REPORT must not emit Finger Up events"
        );

        for evt in [
            tagged(touch, EV_ABS, ABS_MT_TRACKING_ID, 0),
            tagged(touch, EV_ABS, ABS_MT_POSITION_X, 11),
            tagged(touch, EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(touch, EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(touch, EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Motion,
                    ..
                }
            ),
            "the finger must survive the foreign SYN_REPORT and report Motion"
        );
    }
    /// A dropped-event notification from the kernel discards the digitizer's
    /// report state, so in-contact fingers must be lifted explicitly instead of
    /// staying tracked forever.
    #[tokio::test]
    async fn syn_dropped_lifts_tracked_fingers() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        crate::runtime::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let tagged = |kind, code, value| TaggedInputEvent {
            event: event(kind, code, value),
            profile: DeviceProfile::built_in(),
            source: EventSource::Device(SourceId(0)),
        };
        for evt in [
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 7),
            tagged(EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        let down_position = match next_input_event(&mut ry).await {
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                position,
                ..
            } => position,
            other => panic!("expected Finger Down before the drop, got {other:?}"),
        };

        raw_tx
            .send(tagged(EV_SYN, SYN_DROPPED, 0))
            .expect("send raw event");

        match next_input_event(&mut ry).await {
            DeviceEvent::Finger {
                id,
                status: FingerStatus::Up,
                position,
                ..
            } => {
                assert_eq!(id, 7);
                assert_eq!(position, down_position);
            }
            other => panic!("expected Finger Up for the discarded finger, got {other:?}"),
        }

        for evt in [
            tagged(EV_ABS, ABS_MT_TRACKING_ID, 7),
            tagged(EV_ABS, ABS_MT_POSITION_X, 10),
            tagged(EV_ABS, ABS_MT_POSITION_Y, 20),
            tagged(EV_ABS, ABS_MT_PRESSURE, 200),
            tagged(EV_SYN, SYN_REPORT, 0),
        ] {
            raw_tx.send(evt).expect("send raw event");
        }
        assert!(
            matches!(
                next_input_event(&mut ry).await,
                DeviceEvent::Finger {
                    status: FingerStatus::Down,
                    ..
                }
            ),
            "the discarded pipeline must not keep the finger tracked"
        );
    }

    /// Regression: a modifier held on one keyboard when it disconnects must not
    /// latch into keyboard events from another keyboard.
    #[tokio::test]
    async fn modifiers_do_not_leak_between_sources() {
        let (raw_tx, raw_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ty, mut ry) = tokio::sync::mpsc::unbounded_channel();
        let info = DeviceInputInfo {
            proto: TouchProto::MultiB,
            mark: 13,
            mirroring_scheme: (0, 0),
            swapping_scheme: 0,
            startup_rotation: 0,
            gyro_rotation_transform: GyroRotationTransform::default(),
        };
        crate::runtime::spawn(async move {
            let mut rx = raw_rx;
            parse_device_events(
                &mut rx,
                &ty,
                Display {
                    dims: (100, 100),
                    rotation: 0,
                },
                ButtonScheme::Natural,
                info,
            )
            .await;
        });

        let event = |kind, code, value| InputEvent {
            time: libc::timeval {
                tv_sec: 1,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let keyboard = |source, code, value| TaggedInputEvent {
            event: event(EV_KEY, code, value),
            profile: DeviceProfile::keyboard(),
            source: EventSource::Device(source),
        };

        let a = SourceId(0);
        raw_tx
            .send(keyboard(a, KEY_LEFTSHIFT, VAL_PRESS))
            .expect("send shift press");
        let b = SourceId(1);
        raw_tx
            .send(keyboard(b, KEY_SPACE, VAL_PRESS))
            .expect("send space press");

        match ry.recv().await {
            Some(DeviceEvent::Button {
                code: ButtonCode::Keyboard(key),
                ..
            }) => {
                assert_eq!(key.keycode, KEY_SPACE as u32);
                assert_eq!(
                    key.modifiers,
                    Modifiers::default(),
                    "B must not see A's stale Shift"
                );
            }
            other => panic!("expected a keyboard button event, got {other:?}"),
        }
    }
    /// `UserActivity` is throttled to one report per activity interval, and the
    /// reference time is updated only when a report is emitted.
    #[test]
    fn user_activity_is_reported_at_the_activity_interval() {
        let (ty, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let timeval = |tv_sec| libc::timeval { tv_sec, tv_usec: 0 };
        let mut last_activity = 0;

        report_user_activity(&mut last_activity, timeval(60), &ty);
        assert!(matches!(rx.try_recv(), Ok(DeviceEvent::UserActivity)));
        assert_eq!(last_activity, 60);

        report_user_activity(&mut last_activity, timeval(30), &ty);
        assert!(
            rx.try_recv().is_err(),
            "inside the interval must stay quiet"
        );
        assert_eq!(last_activity, 60, "reference time only moves on a report");
    }
}
