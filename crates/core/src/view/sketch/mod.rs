use crate::color::{BLACK, Color, WHITE};
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::{DeviceIdentity as _, DevicePaths as _};
use crate::fl;
use crate::framebuffer::{Framebuffer, Pixmap, UpdateMode};
use crate::geom::{CornerSpec, Point, Rectangle};
use crate::helpers::IsHidden;
use crate::input::{DeviceEvent, FingerStatus};
use crate::settings::Pen;
use crate::unit::{mm_to_px, scale_by_dpi};
use crate::view::common::locate_by_id;
use crate::view::icon::{Icon, load_icon_pixmap};
use crate::view::menu::{Menu, MenuKind};
use crate::view::notification::Notification;
use crate::view::{BORDER_RADIUS_SMALL, SMALL_BAR_HEIGHT};
use crate::view::{Bus, Event, Hub, RenderData, RenderQueue, View};
use crate::view::{EntryId, EntryKind, ID_FEEDER, Id, ViewId};
use anyhow::Error;
use chrono::Local;
use globset::Glob;
use rustc_hash::FxHashMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::PathBuf;
use walkdir::WalkDir;

const FILENAME_PATTERN: &str = "sketch-%Y%m%d_%H%M%S.png";
const ICON_NAME: &str = "enclosed_menu";
// https://oeis.org/A000041
const PEN_SIZES: [i32; 12] = [1, 2, 3, 5, 7, 11, 15, 22, 30, 42, 56, 77];
/// Maximum number of undo (and redo) canvas snapshots kept in [`History`].
const MAX_HISTORY: usize = 10;

/// A canvas snapshot and the file it belongs to, so undo/redo also restores the save target.
struct Snapshot {
    pixmap: Pixmap,
    filename: String,
}

/// Canvas snapshots for undo/redo, taken before each stroke or canvas-wide change.
#[derive(Default)]
struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    fn checkpoint(&mut self, current: &Pixmap, filename: &str) {
        self.commit(Snapshot {
            pixmap: current.clone(),
            filename: filename.to_string(),
        });
    }

    fn commit(&mut self, snapshot: Snapshot) {
        if self.undo.len() == MAX_HISTORY {
            self.undo.remove(0);
        }
        self.undo.push(snapshot);
        self.redo.clear();
    }

    fn undo(&mut self, current: &mut Pixmap, filename: &mut String) -> bool {
        Self::swap(&mut self.undo, &mut self.redo, current, filename)
    }

    fn redo(&mut self, current: &mut Pixmap, filename: &mut String) -> bool {
        Self::swap(&mut self.redo, &mut self.undo, current, filename)
    }

    fn swap(
        from: &mut Vec<Snapshot>,
        to: &mut Vec<Snapshot>,
        current: &mut Pixmap,
        filename: &mut String,
    ) -> bool {
        let Some(previous) = from.pop() else {
            return false;
        };
        to.push(Snapshot {
            pixmap: std::mem::replace(current, previous.pixmap),
            filename: std::mem::replace(filename, previous.filename),
        });
        true
    }
}

struct TouchState {
    pt: Point,
    time: f64,
    radius: f32,
}

impl TouchState {
    fn new(pt: Point, time: f64, radius: f32) -> TouchState {
        TouchState { pt, time, radius }
    }
}

pub struct Sketch {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    pixmap: Pixmap,
    fingers: FxHashMap<i32, TouchState>,
    pen: Pen,
    eraser: bool,
    history: History,
    pending: Option<Pixmap>,
    save_path: PathBuf,
    filename: String,
}

impl Sketch {
    pub fn new(rect: Rectangle, rq: &mut RenderQueue, context: &mut AppContext) -> Sketch {
        let id = ID_FEEDER.next();
        let mut children = Vec::new();
        let dpi = context.device.dpi();
        let small_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let border_radius = scale_by_dpi(BORDER_RADIUS_SMALL, dpi) as i32;
        let pixmap = load_icon_pixmap(ICON_NAME, dpi, &context.device.install_dir()).unwrap();
        let icon_padding = (small_height - pixmap.width.max(pixmap.height) as i32) / 2;
        let width = pixmap.width as i32 + icon_padding;
        let height = pixmap.height as i32 + icon_padding;
        let dx = (small_height - width) / 2;
        let dy = (small_height - height) / 2;
        let icon_rect = rect![
            rect.min.x + dx,
            rect.max.y - dy - height,
            rect.min.x + dx + width,
            rect.max.y - dy
        ];
        let icon = Icon::new(
            ICON_NAME,
            icon_rect,
            Event::ToggleNear(ViewId::TitleMenu, icon_rect),
        )
        .corners(Some(CornerSpec::Uniform(border_radius)));
        children.push(Box::new(icon) as Box<dyn View>);
        let save_path = context
            .library
            .home
            .join(&context.settings.sketch.save_path);
        rq.add(RenderData::new(id, rect, UpdateMode::Full));
        Sketch {
            id,
            rect,
            children,
            pixmap: Pixmap::new(rect.width(), rect.height(), 1),
            fingers: FxHashMap::default(),
            pen: context.settings.sketch.pen.clone(),
            eraser: false,
            history: History::default(),
            pending: None,
            save_path,
            filename: Local::now().format(FILENAME_PATTERN).to_string(),
        }
    }

    fn toggle_title_menu(
        &mut self,
        rect: Rectangle,
        enable: Option<bool>,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        if let Some(index) = locate_by_id(self, ViewId::SketchMenu) {
            if let Some(true) = enable {
                return;
            }

            rq.add(RenderData::expose(
                *self.child(index).rect(),
                UpdateMode::Gui,
            ));
            self.children.remove(index);
        } else {
            if let Some(false) = enable {
                return;
            }

            let glob = Glob::new("**/*.png").unwrap().compile_matcher();
            let mut loadables: Vec<PathBuf> = WalkDir::new(&self.save_path)
                .min_depth(1)
                .into_iter()
                .filter_map(|e| {
                    e.ok()
                        .filter(|e| !e.is_hidden())
                        .and_then(|e| e.path().file_name().map(PathBuf::from))
                })
                .filter(|p| glob.is_match(p))
                .collect();
            loadables.sort_by(|a, b| b.cmp(a));

            let mut sizes = vec![
                EntryKind::CheckBox(
                    "Dynamic".to_string(),
                    EntryId::TogglePenDynamism,
                    self.pen.dynamic,
                ),
                EntryKind::Separator,
            ];

            for s in PEN_SIZES.iter() {
                sizes.push(EntryKind::RadioButton(
                    s.to_string(),
                    EntryId::SetPenSize(*s),
                    self.pen.size == *s,
                ));
            }

            let mut colors = vec![
                EntryKind::RadioButton(
                    "White".to_string(),
                    EntryId::SetPenColor(WHITE),
                    self.pen.color == WHITE,
                ),
                EntryKind::RadioButton(
                    "Black".to_string(),
                    EntryId::SetPenColor(BLACK),
                    self.pen.color == BLACK,
                ),
            ];

            for i in 1..=14 {
                let level = i * 17;
                if i % 7 == 1 {
                    colors.push(EntryKind::Separator);
                }
                let color = Color::Gray(level);
                colors.push(EntryKind::RadioButton(
                    format!("Gray {:02}", i),
                    EntryId::SetPenColor(color),
                    self.pen.color == color,
                ));
            }

            let mut entries = vec![
                EntryKind::SubMenu("Size".to_string(), sizes),
                EntryKind::SubMenu("Color".to_string(), colors),
                EntryKind::CheckBox(fl!("sketch-eraser"), EntryId::TogglePenEraser, self.eraser),
                EntryKind::Separator,
                EntryKind::Command(fl!("sketch-undo"), EntryId::Undo),
                EntryKind::Command(fl!("sketch-redo"), EntryId::Redo),
                EntryKind::Separator,
                EntryKind::Command("Save".to_string(), EntryId::Save),
                EntryKind::Command("Refresh".to_string(), EntryId::Refresh),
                EntryKind::Command("New".to_string(), EntryId::New),
                EntryKind::Command("Quit".to_string(), EntryId::Quit),
            ];

            if !loadables.is_empty() {
                entries.insert(
                    entries.len() - 1,
                    EntryKind::SubMenu(
                        "Load".to_string(),
                        loadables
                            .into_iter()
                            .map(|e| {
                                EntryKind::Command(
                                    e.to_string_lossy().into_owned(),
                                    EntryId::Load(e),
                                )
                            })
                            .collect(),
                    ),
                );
            }

            let sketch_menu = Menu::new(
                rect,
                ViewId::SketchMenu,
                MenuKind::Contextual,
                entries,
                context,
            );
            rq.add(RenderData::new(
                sketch_menu.id(),
                *sketch_menu.rect(),
                UpdateMode::Gui,
            ));
            self.children.push(Box::new(sketch_menu) as Box<dyn View>);
        }
    }

    fn finish_pending(&mut self) {
        if let Some(pixmap) = self.pending.take()
            && pixmap.data != self.pixmap.data
        {
            self.history.commit(Snapshot {
                pixmap,
                filename: self.filename.clone(),
            });
        }
    }

    fn load(&mut self, filename: &PathBuf) -> Result<(), Error> {
        let path = self.save_path.join(filename);
        let file = File::open(path)?;
        let decoder = png::Decoder::new(BufReader::new(file));
        let mut reader = decoder.read_info()?;
        let mut pixmap = self.pixmap.clone();
        reader.next_frame(pixmap.data_mut())?;
        self.finish_pending();
        self.history.checkpoint(&self.pixmap, &self.filename);
        self.pixmap = pixmap;
        self.filename = filename.to_string_lossy().into_owned();
        if !self.fingers.is_empty() {
            self.pending = Some(self.pixmap.clone());
        }
        Ok(())
    }

    fn save(&self) -> Result<(), Error> {
        if !self.save_path.exists() {
            fs::create_dir_all(&self.save_path)?;
        }
        let path = self.save_path.join(&self.filename);
        self.pixmap.save(&path.to_string_lossy().into_owned())?;
        Ok(())
    }

    fn stroke_color(&self) -> Color {
        if self.eraser { WHITE } else { self.pen.color }
    }

    fn quit(&self, hub: &Hub, context: &AppContext) {
        hub.send(
            (Event::ImportLibrary {
                library_index: Some(context.settings.selected_library),
                force: false,
            })
            .into(),
        )
        .ok();
    }
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn draw_segment(
    pixmap: &mut Pixmap,
    ts: &mut TouchState,
    position: Point,
    time: f64,
    pen: &Pen,
    color: Color,
    dpi: u16,
    id: Id,
    fb_rect: &Rectangle,
    rq: &mut RenderQueue,
) {
    let (start_radius, end_radius) = if pen.dynamic {
        if time > ts.time {
            let d = vec2!((position.x - ts.pt.x) as f32, (position.y - ts.pt.y) as f32).length();
            let speed = d / (time - ts.time) as f32;
            let min_speed_px = mm_to_px(pen.min_speed, dpi);
            let max_speed_px = mm_to_px(pen.max_speed, dpi);
            let base_radius = pen.size as f32 / 2.0;
            let radius = base_radius
                * (1.0
                    + (pen.amplitude / base_radius) * speed.clamp(min_speed_px, max_speed_px)
                        / (max_speed_px - min_speed_px));
            (ts.radius, radius)
        } else {
            (ts.radius, ts.radius)
        }
    } else {
        let radius = pen.size as f32 / 2.0;
        (radius, radius)
    };

    let rect = Rectangle::from_segment(
        ts.pt,
        position,
        start_radius.ceil() as i32,
        end_radius.ceil() as i32,
    );

    pixmap.draw_segment(ts.pt, position, start_radius, end_radius, color);

    if let Some(render_rect) = rect.intersection(fb_rect) {
        rq.add(RenderData::no_wait(id, render_rect, UpdateMode::FastMono));
    }

    ts.pt = position;
    ts.time = time;
    ts.radius = end_radius;
}

#[async_trait::async_trait(?Send)]
impl View for Sketch {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    async fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::Device(DeviceEvent::Finger {
                status: FingerStatus::Motion,
                id,
                position,
                time,
            }) => {
                let color = self.stroke_color();
                if let Some(ts) = self.fingers.get_mut(&id) {
                    draw_segment(
                        &mut self.pixmap,
                        ts,
                        position,
                        time,
                        &self.pen,
                        color,
                        context.device.dpi(),
                        self.id,
                        &self.rect,
                        rq,
                    );
                }
                true
            }
            Event::Device(DeviceEvent::Finger {
                status: FingerStatus::Down,
                id,
                position,
                time,
            }) => {
                if self.fingers.is_empty() {
                    self.pending = Some(self.pixmap.clone());
                }
                let radius = self.pen.size as f32 / 2.0;
                self.fingers
                    .insert(id, TouchState::new(position, time, radius));
                true
            }
            Event::Device(DeviceEvent::Finger {
                status: FingerStatus::Up,
                id,
                position,
                time,
            }) => {
                let color = self.stroke_color();
                if let Some(ts) = self.fingers.get_mut(&id) {
                    draw_segment(
                        &mut self.pixmap,
                        ts,
                        position,
                        time,
                        &self.pen,
                        color,
                        context.device.dpi(),
                        self.id,
                        &self.rect,
                        rq,
                    );
                }
                self.fingers.remove(&id);
                if self.fingers.is_empty() {
                    self.finish_pending();
                }
                true
            }
            Event::ToggleNear(ViewId::TitleMenu, rect) => {
                self.toggle_title_menu(rect, None, rq, context);
                true
            }
            Event::Select(EntryId::SetPenSize(size)) => {
                self.pen.size = size;
                true
            }
            Event::Select(EntryId::SetPenColor(color)) => {
                self.pen.color = color;
                true
            }
            Event::Select(EntryId::TogglePenDynamism) => {
                self.pen.dynamic = !self.pen.dynamic;
                true
            }
            Event::Select(EntryId::TogglePenEraser) => {
                self.eraser = !self.eraser;
                true
            }
            Event::Select(EntryId::Undo) => {
                if self.history.undo(&mut self.pixmap, &mut self.filename) {
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Select(EntryId::Redo) => {
                if self.history.redo(&mut self.pixmap, &mut self.filename) {
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Select(EntryId::Load(ref name)) => {
                if let Err(e) = self.load(name) {
                    let msg = format!("Couldn't load sketch: {}).", e);
                    let notif = Notification::new(None, msg, false, hub, rq, context);
                    self.children.push(Box::new(notif) as Box<dyn View>);
                } else {
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                }
                true
            }
            Event::Select(EntryId::Refresh) => {
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                true
            }
            Event::Select(EntryId::New) => {
                self.finish_pending();
                self.history.checkpoint(&self.pixmap, &self.filename);
                self.pixmap.clear(WHITE);
                self.filename = Local::now().format(FILENAME_PATTERN).to_string();
                if !self.fingers.is_empty() {
                    self.pending = Some(self.pixmap.clone());
                }
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                true
            }
            Event::Select(EntryId::Save) => {
                let mut msg = match self.save() {
                    Err(e) => Some(format!("Can't save sketch: {}.", e)),
                    Ok(..) => {
                        if context.settings.sketch.notify_success {
                            Some(format!("Saved {}.", self.filename))
                        } else {
                            None
                        }
                    }
                };
                if let Some(msg) = msg.take() {
                    let notif = Notification::new(None, msg, false, hub, rq, context);
                    self.children.push(Box::new(notif) as Box<dyn View>);
                }
                true
            }
            Event::Select(EntryId::Quit) => {
                self.quit(hub, context);
                hub.send((Event::Back).into()).ok();
                true
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context), fields(rect = ?rect)))]
    fn render(&self, context: &mut AppContext, rect: Rectangle) {
        let fb = context.device.framebuffer_mut();
        fb.draw_framed_pixmap_halftone(&self.pixmap, &rect, rect.min);
    }

    fn render_rect(&self, rect: &Rectangle) -> Rectangle {
        rect.intersection(&self.rect).unwrap_or(self.rect)
    }

    fn might_rotate(&self) -> bool {
        false
    }

    fn is_background(&self) -> bool {
        true
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(value: u8) -> Pixmap {
        let mut pixmap = Pixmap::new(1, 1, 1);
        pixmap.data_mut()[0] = value;
        pixmap
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut history = History::default();
        let mut current = canvas(1);
        let mut filename = "a".to_string();
        history.checkpoint(&current, &filename);
        current = canvas(2);
        filename = "b".to_string();

        assert!(history.undo(&mut current, &mut filename));
        assert_eq!(current.data[0], 1);
        assert_eq!(filename, "a");
        assert!(!history.undo(&mut current, &mut filename));
        assert!(history.redo(&mut current, &mut filename));
        assert_eq!(current.data[0], 2);
        assert_eq!(filename, "b");
        assert!(!history.redo(&mut current, &mut filename));
    }

    #[test]
    fn checkpoint_clears_redo_and_caps_depth() {
        let mut history = History::default();
        let mut current = canvas(0);
        let mut filename = "a".to_string();
        history.checkpoint(&current, &filename);
        assert!(history.undo(&mut current, &mut filename));
        history.checkpoint(&current, &filename);
        assert!(history.redo.is_empty());

        (0..MAX_HISTORY as u8 + 5).for_each(|i| history.checkpoint(&canvas(i), &i.to_string()));
        assert_eq!(history.undo.len(), MAX_HISTORY);
        assert_eq!(history.undo[0].pixmap.data[0], 5);
    }
}
