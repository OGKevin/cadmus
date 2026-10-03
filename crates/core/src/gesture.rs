use crate::geom::{Axis, DiagDir, Dir, Point, Vec2, elbow, nearest_segment_point};
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use crate::unit::mm_to_px;
use crate::view::Event;
use rustc_hash::FxHashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

pub const TAP_JITTER_MM: f32 = 6.0;
pub const HOLD_JITTER_MM: f32 = 1.5;
pub const HOLD_DELAY_SHORT: Duration = Duration::from_millis(666);
pub const HOLD_DELAY_LONG: Duration = Duration::from_millis(1333);

#[derive(Debug, Copy, Clone)]
pub enum GestureEvent {
    Tap(Point),
    MultiTap([Point; 2]),
    Swipe {
        dir: Dir,
        start: Point,
        end: Point,
    },
    SlantedSwipe {
        dir: DiagDir,
        start: Point,
        end: Point,
    },
    MultiSwipe {
        dir: Dir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Arrow {
        dir: Dir,
        start: Point,
        end: Point,
    },
    MultiArrow {
        dir: Dir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Corner {
        dir: DiagDir,
        start: Point,
        end: Point,
    },
    MultiCorner {
        dir: DiagDir,
        starts: [Point; 2],
        ends: [Point; 2],
    },
    Pinch {
        axis: Axis,
        center: Point,
        factor: f32,
    },
    Spread {
        axis: Axis,
        center: Point,
        factor: f32,
    },
    Rotate {
        center: Point,
        quarter_turns: i8,
        angle: f32,
    },
    Cross(Point),
    Diamond(Point),
    HoldFingerShort(Point, i32),
    HoldFingerLong(Point, i32),
    HoldButtonShort(ButtonCode),
    HoldButtonLong(ButtonCode),
}

impl fmt::Display for GestureEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            GestureEvent::Tap(pt) => write!(f, "Tap {}", pt),
            GestureEvent::MultiTap(pts) => write!(f, "Multitap {} {}", pts[0], pts[1]),
            GestureEvent::Swipe { dir, .. } => write!(f, "Swipe {}", dir),
            GestureEvent::SlantedSwipe { dir, .. } => write!(f, "SlantedSwipe {}", dir),
            GestureEvent::MultiSwipe { dir, .. } => write!(f, "Multiswipe {}", dir),
            GestureEvent::Arrow { dir, .. } => write!(f, "Arrow {}", dir),
            GestureEvent::MultiArrow { dir, .. } => write!(f, "Multiarrow {}", dir),
            GestureEvent::Corner { dir, .. } => write!(f, "Corner {}", dir),
            GestureEvent::MultiCorner { dir, .. } => write!(f, "Multicorner {}", dir),
            GestureEvent::Pinch {
                axis,
                center,
                factor,
                ..
            } => write!(f, "Pinch {} {} {:.2}", axis, center, factor),
            GestureEvent::Spread {
                axis,
                center,
                factor,
                ..
            } => write!(f, "Spread {} {} {:.2}", axis, center, factor),
            GestureEvent::Rotate {
                center,
                quarter_turns,
                ..
            } => write!(f, "Rotate {} {}", center, *quarter_turns as i32 * 90),
            GestureEvent::Cross(pt) => write!(f, "Cross {}", pt),
            GestureEvent::Diamond(pt) => write!(f, "Diamond {}", pt),
            GestureEvent::HoldFingerShort(pt, id) => write!(f, "Short-held finger {} {}", id, pt),
            GestureEvent::HoldFingerLong(pt, id) => write!(f, "Long-held finger {} {}", id, pt),
            GestureEvent::HoldButtonShort(code) => write!(f, "Short-held button {:?}", code),
            GestureEvent::HoldButtonLong(code) => write!(f, "Long-held button {:?}", code),
        }
    }
}

#[derive(Debug)]
struct TouchState {
    time: f64,
    held: bool,
    positions: Vec<Point>,
}

/// Gesture recognition output plus the [`crate::runtime::Job`] that owns the parser.
///
/// Hold timers are subtasks of that job; they are cancelled when a contact ends,
/// when the pipeline [`CancellationToken`] fires, or when the job is dropped.
pub struct GesturePipeline {
    pub events: UnboundedReceiver<Event>,
    job: crate::runtime::Job<()>,
}

impl GesturePipeline {
    pub fn start(rx: UnboundedReceiver<DeviceEvent>, dpi: u16) -> Self {
        let (ty, events) = tokio::sync::mpsc::unbounded_channel();
        let job = crate::runtime::Job::spawn(move |cancel| async move {
            parse_gesture_events(rx, ty, dpi, cancel).await;
        });
        Self { events, job }
    }

    pub fn into_parts(self) -> (UnboundedReceiver<Event>, crate::runtime::Job<()>) {
        (self.events, self.job)
    }
}

async fn parse_gesture_events(
    mut rx: UnboundedReceiver<DeviceEvent>,
    ty: UnboundedSender<Event>,
    dpi: u16,
    cancel: CancellationToken,
) {
    let contacts: Arc<Mutex<FxHashMap<i32, TouchState>>> =
        Arc::new(Mutex::new(FxHashMap::default()));
    let buttons: Arc<Mutex<FxHashMap<ButtonCode, f64>>> =
        Arc::new(Mutex::new(FxHashMap::default()));
    let segments: Arc<Mutex<Vec<Vec<Point>>>> = Arc::new(Mutex::new(Vec::new()));
    let tap_jitter = mm_to_px(TAP_JITTER_MM, dpi);
    let hold_jitter = mm_to_px(HOLD_JITTER_MM, dpi);
    let mut finger_holds: FxHashMap<i32, CancellationToken> = FxHashMap::default();
    let mut button_holds: FxHashMap<ButtonCode, CancellationToken> = FxHashMap::default();

    loop {
        let evt = tokio::select! {
            () = cancel.cancelled() => break,
            recv = rx.recv() => match recv {
                Some(evt) => evt,
                None => break,
            },
        };

        ty.send(Event::Device(evt)).ok();
        match evt {
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                position,
                id,
                time,
            } => {
                let mut ct = contacts.lock().unwrap();
                ct.insert(
                    id,
                    TouchState {
                        time,
                        held: false,
                        positions: vec![position],
                    },
                );
                tracing::debug!(
                    id,
                    position = ?position,
                    time,
                    short_ms = HOLD_DELAY_SHORT.as_millis(),
                    long_ms = HOLD_DELAY_LONG.as_millis(),
                    "finger hold timer armed"
                );
                disarm_hold(&mut finger_holds, id);
                let hold_cancel = CancellationToken::new();
                finger_holds.insert(id, hold_cancel.clone());
                let ty = ty.clone();
                let contacts = contacts.clone();
                let segments = segments.clone();
                tokio::spawn(run_finger_hold(
                    ty,
                    contacts,
                    segments,
                    hold_cancel,
                    FingerHoldArm {
                        id,
                        position,
                        time,
                        hold_jitter,
                    },
                ));
            }
            DeviceEvent::Finger {
                status: FingerStatus::Motion,
                position,
                id,
                ..
            } => {
                let mut ct = contacts.lock().unwrap();
                if let Some(ref mut ts) = ct.get_mut(&id) {
                    ts.positions.push(position);
                }
            }
            DeviceEvent::Finger {
                status: FingerStatus::Up,
                position,
                id,
                ..
            } => {
                disarm_hold(&mut finger_holds, id);
                let mut ct = contacts.lock().unwrap();
                let mut sg = segments.lock().unwrap();
                if let Some(mut ts) = ct.remove(&id) {
                    tracing::debug!(
                        id,
                        position = ?position,
                        was_held = ts.held,
                        "finger hold cleared on up"
                    );
                    if !ts.held {
                        ts.positions.push(position);
                        sg.push(ts.positions);
                    }
                } else {
                    tracing::trace!(
                        id,
                        position = ?position,
                        "finger up with no active contact"
                    );
                }
                if ct.is_empty() && !sg.is_empty() {
                    let len = sg.len();
                    if len == 1 {
                        ty.send(Event::Gesture(interpret_segment(
                            &sg.pop().unwrap(),
                            tap_jitter,
                        )))
                        .ok();
                    } else if len == 2 {
                        let ge1 = interpret_segment(&sg.pop().unwrap(), tap_jitter);
                        let ge2 = interpret_segment(&sg.pop().unwrap(), tap_jitter);
                        match (ge1, ge2) {
                            (GestureEvent::Tap(c1), GestureEvent::Tap(c2)) => {
                                ty.send(Event::Gesture(GestureEvent::MultiTap([c1, c2])))
                                    .ok();
                            }
                            (
                                GestureEvent::Swipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::Swipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiSwipe {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Swipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::Swipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2.opposite() => {
                                let center = (s1 + s2) / 2;
                                let ds = (s2 - s1).length();
                                let de = (e2 - e1).length();
                                let factor = de / ds;
                                if factor < 1.0 {
                                    ty.send(Event::Gesture(GestureEvent::Pinch {
                                        axis: d1.axis(),
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                } else {
                                    ty.send(Event::Gesture(GestureEvent::Spread {
                                        axis: d1.axis(),
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                }
                            }
                            (
                                GestureEvent::SlantedSwipe {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                    ..
                                },
                                GestureEvent::SlantedSwipe {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                    ..
                                },
                            ) if d1 == d2.opposite() => {
                                let center = (s1 + s2) / 2;
                                let ds = (s2 - s1).length();
                                let de = (e2 - e1).length();
                                let factor = de / ds;
                                if factor < 1.0 {
                                    ty.send(Event::Gesture(GestureEvent::Pinch {
                                        axis: Axis::Diagonal,
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                } else {
                                    ty.send(Event::Gesture(GestureEvent::Spread {
                                        axis: Axis::Diagonal,
                                        center,
                                        factor,
                                    }))
                                    .ok();
                                }
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s2,
                                    end: e2,
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s2,
                                    end: e2,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s1,
                                    end: e1,
                                },
                            ) if s1.x < s2.x => {
                                ty.send(Event::Gesture(GestureEvent::Cross(
                                    (s1 + e1 + s2 + e2) / 4,
                                )))
                                .ok();
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s2,
                                    end: e2,
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    dir: Dir::East,
                                    start: s2,
                                    end: e2,
                                },
                                GestureEvent::Arrow {
                                    dir: Dir::West,
                                    start: s1,
                                    end: e1,
                                },
                            ) if s1.x < s2.x => {
                                ty.send(Event::Gesture(GestureEvent::Diamond(
                                    (s1 + e1 + s2 + e2) / 4,
                                )))
                                .ok();
                            }
                            (
                                GestureEvent::Arrow {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Arrow {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiArrow {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Corner {
                                    dir: d1,
                                    start: s1,
                                    end: e1,
                                },
                                GestureEvent::Corner {
                                    dir: d2,
                                    start: s2,
                                    end: e2,
                                },
                            ) if d1 == d2 => {
                                ty.send(Event::Gesture(GestureEvent::MultiCorner {
                                    dir: d1,
                                    starts: [s1, s2],
                                    ends: [e1, e2],
                                }))
                                .ok();
                            }
                            (
                                GestureEvent::Tap(c),
                                GestureEvent::Swipe {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Swipe {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            )
                            | (
                                GestureEvent::Tap(c),
                                GestureEvent::Arrow {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Arrow {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            )
                            | (
                                GestureEvent::Tap(c),
                                GestureEvent::Corner {
                                    start: s, end: e, ..
                                },
                            )
                            | (
                                GestureEvent::Corner {
                                    start: s, end: e, ..
                                },
                                GestureEvent::Tap(c),
                            ) => {
                                // Angle are positive in the counter clockwise direction.
                                let angle = ((e - c).angle() - (s - c).angle()).to_degrees();
                                let quarter_turns = (angle / 90.0).round() as i8;
                                ty.send(Event::Gesture(GestureEvent::Rotate {
                                    angle,
                                    quarter_turns,
                                    center: c,
                                }))
                                .ok();
                            }
                            _ => (),
                        }
                    } else {
                        sg.clear();
                    }
                }
            }
            DeviceEvent::Button {
                status: ButtonStatus::Pressed,
                code,
                time,
            } => {
                let mut bt = buttons.lock().unwrap();
                bt.insert(code, time);
                tracing::debug!(
                    code = ?code,
                    time,
                    short_ms = HOLD_DELAY_SHORT.as_millis(),
                    long_ms = HOLD_DELAY_LONG.as_millis(),
                    "button hold timer armed"
                );
                disarm_hold(&mut button_holds, code);
                let hold_cancel = CancellationToken::new();
                button_holds.insert(code, hold_cancel.clone());
                let ty = ty.clone();
                let buttons = buttons.clone();
                tokio::spawn(run_button_hold(ty, buttons, hold_cancel, code, time));
            }
            DeviceEvent::Button {
                status: ButtonStatus::Released,
                code,
                ..
            } => {
                disarm_hold(&mut button_holds, code);
                let mut bt = buttons.lock().unwrap();
                let cleared = bt.remove(&code).is_some();
                tracing::debug!(code = ?code, cleared, "button hold cleared on release");
            }
            _ => (),
        }
    }

    disarm_all_holds(&mut finger_holds);
    disarm_all_holds(&mut button_holds);
}

fn disarm_hold<K: std::hash::Hash + Eq>(holds: &mut FxHashMap<K, CancellationToken>, key: K) {
    if let Some(token) = holds.remove(&key) {
        token.cancel();
    }
}

fn disarm_all_holds<K: std::hash::Hash + Eq>(holds: &mut FxHashMap<K, CancellationToken>) {
    for (_, token) in holds.drain() {
        token.cancel();
    }
}

async fn sleep_hold(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        () = cancel.cancelled() => false,
        () = tokio::time::sleep(duration) => true,
    }
}

struct FingerHoldArm {
    id: i32,
    position: Point,
    time: f64,
    hold_jitter: f32,
}

async fn run_finger_hold(
    ty: UnboundedSender<Event>,
    contacts: Arc<Mutex<FxHashMap<i32, TouchState>>>,
    segments: Arc<Mutex<Vec<Vec<Point>>>>,
    cancel: CancellationToken,
    arm: FingerHoldArm,
) {
    let FingerHoldArm {
        id,
        position,
        time,
        hold_jitter,
    } = arm;
    if !sleep_hold(&cancel, HOLD_DELAY_SHORT).await {
        return;
    }
    let mut held = false;
    {
        let mut ct = contacts.lock().unwrap();
        let sg = segments.lock().unwrap();
        if ct.len() > 1 || !sg.is_empty() {
            tracing::trace!(
                id,
                contacts = ct.len(),
                segments = sg.len(),
                "hold finger short cancelled"
            );
            return;
        }
        if let Some(ts) = ct.get(&id) {
            let tp = &ts.positions;
            if ts.time == time
                && (tp[tp.len() - 1] - position).length() < hold_jitter
                && (tp[tp.len() / 2] - position).length() < hold_jitter
            {
                held = true;
                tracing::debug!(
                    id,
                    position = ?position,
                    time,
                    "hold finger short fired"
                );
                ty.send(Event::Gesture(GestureEvent::HoldFingerShort(position, id)))
                    .ok();
            } else {
                tracing::trace!(
                    id,
                    position = ?position,
                    time,
                    "hold finger short cancelled"
                );
            }
        } else {
            tracing::trace!(
                id,
                position = ?position,
                time,
                "hold finger short cancelled"
            );
        }
        if held {
            if let Some(ts) = ct.get_mut(&id) {
                ts.held = true;
            }
        } else {
            return;
        }
    }
    if !sleep_hold(&cancel, HOLD_DELAY_LONG - HOLD_DELAY_SHORT).await {
        return;
    }
    let mut ct = contacts.lock().unwrap();
    let sg = segments.lock().unwrap();
    if ct.len() > 1 || !sg.is_empty() {
        tracing::trace!(
            id,
            contacts = ct.len(),
            segments = sg.len(),
            "hold finger long cancelled"
        );
        return;
    }
    if let Some(ts) = ct.get_mut(&id) {
        let tp = &ts.positions;
        if ts.time == time
            && (tp[tp.len() - 1] - position).length() < hold_jitter
            && (tp[tp.len() / 2] - position).length() < hold_jitter
        {
            tracing::debug!(
                id,
                position = ?position,
                time,
                "hold finger long fired"
            );
            ty.send(Event::Gesture(GestureEvent::HoldFingerLong(position, id)))
                .ok();
        } else {
            tracing::trace!(
                id,
                position = ?position,
                time,
                "hold finger long cancelled"
            );
        }
    } else {
        tracing::trace!(
            id,
            position = ?position,
            time,
            "hold finger long cancelled"
        );
    }
}

async fn run_button_hold(
    ty: UnboundedSender<Event>,
    buttons: Arc<Mutex<FxHashMap<ButtonCode, f64>>>,
    cancel: CancellationToken,
    code: ButtonCode,
    time: f64,
) {
    if !sleep_hold(&cancel, HOLD_DELAY_SHORT).await {
        return;
    }
    {
        let bt = buttons.lock().unwrap();
        match bt.get(&code) {
            Some(&initial_time) if initial_time == time => {
                tracing::debug!(code = ?code, time, "hold button short fired");
                ty.send(Event::Gesture(GestureEvent::HoldButtonShort(code)))
                    .ok();
            }
            Some(&initial_time) => {
                tracing::trace!(
                    code = ?code,
                    time,
                    initial_time,
                    "hold button short cancelled"
                );
            }
            None => {
                tracing::trace!(code = ?code, time, "hold button short cancelled");
            }
        }
    }
    if !sleep_hold(&cancel, HOLD_DELAY_LONG - HOLD_DELAY_SHORT).await {
        return;
    }
    let bt = buttons.lock().unwrap();
    match bt.get(&code) {
        Some(&initial_time) if initial_time == time => {
            tracing::debug!(code = ?code, time, "hold button long fired");
            ty.send(Event::Gesture(GestureEvent::HoldButtonLong(code)))
                .ok();
        }
        Some(&initial_time) => {
            tracing::trace!(
                code = ?code,
                time,
                initial_time,
                "hold button long cancelled"
            );
        }
        None => {
            tracing::trace!(code = ?code, time, "hold button long cancelled");
        }
    }
}

fn interpret_segment(sp: &[Point], tap_jitter: f32) -> GestureEvent {
    let a = sp[0];
    let b = sp[sp.len() - 1];
    let ab = b - a;
    let d = ab.length();
    if d < tap_jitter {
        GestureEvent::Tap(a)
    } else {
        let p = sp[elbow(sp)];
        let (n, p) = {
            let p: Vec2 = p.into();
            let (n, _) = nearest_segment_point(p, a.into(), b.into());
            (n, p)
        };
        let np = p - n;
        let ds = np.length();
        if ds > d / 5.0 {
            let g = (np.x as f32 / np.y as f32).abs();
            if g < 0.5 || g > 2.0 {
                GestureEvent::Arrow {
                    dir: np.dir(),
                    start: a,
                    end: b,
                }
            } else {
                GestureEvent::Corner {
                    dir: np.diag_dir(),
                    start: a,
                    end: b,
                }
            }
        } else {
            let g = (ab.x as f32 / ab.y as f32).abs();
            if g < 0.5 || g > 2.0 {
                GestureEvent::Swipe {
                    start: a,
                    end: b,
                    dir: ab.dir(),
                }
            } else {
                GestureEvent::SlantedSwipe {
                    start: a,
                    end: b,
                    dir: ab.diag_dir(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;
    use crate::input::{DeviceEvent, FingerStatus};
    use std::time::Duration;
    use tokio::sync::mpsc;

    async fn next_gesture(
        events: &mut UnboundedReceiver<Event>,
        within: Duration,
    ) -> Option<GestureEvent> {
        let deadline = tokio::time::Instant::now() + within;
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, events.recv()).await {
                Ok(Some(Event::Gesture(gesture))) => return Some(gesture),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return None,
            }
        }
        None
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finger_hold_short_fires_when_contact_timestamp_unchanged() {
        let (device_tx, device_rx) = mpsc::unbounded_channel();
        let pipeline = GesturePipeline::start(device_rx, 300);
        let mut events = pipeline.events;
        let position = Point::new(10, 10);
        let down_time = 42.0;

        device_tx
            .send(DeviceEvent::Finger {
                id: 1,
                time: down_time,
                status: FingerStatus::Down,
                position,
            })
            .unwrap();

        let gesture =
            next_gesture(&mut events, HOLD_DELAY_SHORT + Duration::from_millis(250)).await;

        assert!(matches!(
            gesture,
            Some(GestureEvent::HoldFingerShort(pt, 1)) if pt == position
        ));

        drop(device_tx);
        let _ = pipeline.job.join(Duration::from_secs(1)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finger_hold_short_not_fired_after_release() {
        let (device_tx, device_rx) = mpsc::unbounded_channel();
        let pipeline = GesturePipeline::start(device_rx, 300);
        let mut events = pipeline.events;
        let position = Point::new(5, 5);

        device_tx
            .send(DeviceEvent::Finger {
                id: 2,
                time: 1.0,
                status: FingerStatus::Down,
                position,
            })
            .unwrap();
        device_tx
            .send(DeviceEvent::Finger {
                id: 2,
                time: 1.1,
                status: FingerStatus::Up,
                position,
            })
            .unwrap();

        let gesture =
            next_gesture(&mut events, HOLD_DELAY_SHORT + Duration::from_millis(250)).await;

        assert!(!matches!(
            gesture,
            Some(GestureEvent::HoldFingerShort(_, 2))
        ));

        drop(device_tx);
        let _ = pipeline.job.join(Duration::from_secs(1)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finger_hold_short_uses_latest_down_timestamp() {
        let (device_tx, device_rx) = mpsc::unbounded_channel();
        let pipeline = GesturePipeline::start(device_rx, 300);
        let mut events = pipeline.events;
        let position = Point::new(0, 0);

        device_tx
            .send(DeviceEvent::Finger {
                id: 3,
                time: 1.0,
                status: FingerStatus::Down,
                position,
            })
            .unwrap();
        device_tx
            .send(DeviceEvent::Finger {
                id: 3,
                time: 9.0,
                status: FingerStatus::Down,
                position,
            })
            .unwrap();

        let gesture =
            next_gesture(&mut events, HOLD_DELAY_SHORT + Duration::from_millis(250)).await;

        assert!(matches!(
            gesture,
            Some(GestureEvent::HoldFingerShort(pt, 3)) if pt == position
        ));

        drop(device_tx);
        let _ = pipeline.job.join(Duration::from_secs(1)).await;
    }
}
