//! Software-rendering host for Slint views.
//!
//! A [`Session`] owns a generated component and its `MinimalSoftwareWindow`.
//! Device events are dispatched synchronously. Views queue callback results and
//! drain them after dispatch. The session pumps timers and captures changed
//! pixels into a retained frame, which the view blits during the normal render
//! pass.
//!
//! Cadmus drives Slint from its view loop rather than running Slint's event
//! loop. Slint's platform, timers, and font collection are thread-local, so a
//! session must be opened and used on the thread that polls the view tree. The
//! session methods advance timers and animations on that thread.
//!
//! Components generated from `ui/cadmus.slint` are in [`super::generated`].
//! Fonts are loaded from the install directory. CJK families are script
//! fallbacks when those files exist, and Noto Sans covers Cyrillic.
//!
//! `i_slint_common` and `i_slint_core` are referenced so `image-decoders` and
//! `svg` stay on the Slint core the generated UI calls. Without that edge,
//! embedded SVG icons do not link.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rgb::FromSlice as _;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};

use crate::framebuffer::{Framebuffer, Pixmap, UpdateMode};
use crate::geom::{Point, Rectangle};
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use crate::view::{Event, Hub};

use i_slint_common as _;
use i_slint_core as _;

thread_local! {
    static PLATFORM_READY: Cell<bool> = const { Cell::new(false) };
    static WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    /// Last `fonts/` directory registered on this thread. Startup and settings
    /// share one install dir, so the second open skips the scan.
    static LOADED_FONTS: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Epoch for [`SoftwarePlatform::duration_since_start`].
    static CLOCK_START: Cell<Option<Instant>> = const { Cell::new(None) };
    #[cfg(test)]
    static CLOCK_OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

const APP_FAMILY: &str = "Noto Sans";
const CJK_FALLBACK_FAMILIES: &[&str] = &[
    "Noto Sans JP",
    "Noto Sans KR",
    "Noto Sans SC",
    "Noto Sans TC",
];
const CJK_SCRIPTS: &[[u8; 4]] = &[*b"Hani", *b"Hira", *b"Kana", *b"Hang", *b"Bopo"];

fn monotonic_now() -> Duration {
    let start = CLOCK_START.with(|cell| match cell.get() {
        Some(start) => start,
        None => {
            let start = Instant::now();
            cell.set(Some(start));
            start
        }
    });
    let elapsed = Instant::now().saturating_duration_since(start);
    #[cfg(test)]
    let elapsed = elapsed.saturating_add(CLOCK_OFFSET.with(Cell::get));
    elapsed
}

/// Moves this thread's Slint clock forward by `by`.
///
/// Hold tests add the interval here and then pump. The next timer update sees
/// the added time, so the test does not sleep.
#[cfg(test)]
pub(crate) fn advance_test_clock(by: Duration) {
    let _ = monotonic_now();
    CLOCK_OFFSET.with(|cell| cell.set(cell.get().saturating_add(by)));
}

struct SoftwarePlatform;

/// Window whose frame buffer is kept between renders.
///
/// Slint then repaints only the region that changed instead of the whole window.
fn reused_window() -> Rc<MinimalSoftwareWindow> {
    MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer)
}

impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = reused_window();
        WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
        Ok(window)
    }

    fn duration_since_start(&self) -> Duration {
        monotonic_now()
    }
}

/// One Slint component rendered into the Cadmus framebuffer.
pub struct Session<C: ComponentHandle> {
    component: C,
    window: Rc<MinimalSoftwareWindow>,
    /// The current frame. Slint renders into it in place; with
    /// `RepaintBufferType::ReusedBuffer` it keeps the pixels of every area a
    /// frame did not repaint, so it is always the complete window.
    frame: RefCell<Pixmap>,
    /// Changed rectangles accumulated until the view queues its next render.
    dirty: RefCell<Vec<Rectangle>>,
    /// When the wakeup already sent to the hub is due. Keeps one pending
    /// wakeup per deadline instead of one per input.
    wake: Cell<Option<Instant>>,
    scale: f32,
    width: u32,
    height: u32,
}

impl<C: ComponentHandle> Session<C> {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn open(dpi: u16, width: u32, height: u32, build: impl FnOnce() -> Result<C>) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("slint framebuffer is empty");
        }

        ensure_platform()?;
        let component = match build() {
            Ok(component) => component,
            Err(error) => {
                WINDOW.with(|slot| slot.borrow_mut().take());
                return Err(error);
            }
        };
        let window = WINDOW
            .with(|slot| slot.borrow_mut().take())
            .context("slint window was not installed")?;

        let scale = super::pixel_scale(dpi);
        if let Err(error) = component.window().dispatch_event_with_result(
            slint::platform::WindowEvent::ScaleFactorChanged {
                scale_factor: scale,
            },
        ) {
            let _ = component.hide();
            return Err(error).context("failed to set slint scale");
        }
        window.set_size(slint::PhysicalSize::new(width, height));
        if let Err(error) = component.show() {
            let _ = component.hide();
            return Err(error).context("failed to show slint window");
        }
        super::apply_language();

        Ok(Self {
            component,
            window,
            frame: RefCell::new(Pixmap::new(width, height, 3)),
            dirty: RefCell::new(Vec::new()),
            wake: Cell::new(None),
            scale,
            width,
            height,
        })
    }

    /// Opens a component using the supplied font bytes as the UI font.
    #[cfg(test)]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(font_bytes, build),
            fields(dpi = dpi, width = width, height = height)
        )
    )]
    pub(super) fn open_with_app_font(
        dpi: u16,
        width: u32,
        height: u32,
        font_bytes: &[u8],
        build: impl FnOnce(&str) -> Result<C>,
    ) -> Result<Self> {
        let family = register_font(font_bytes)?;
        Self::open(dpi, width, height, || build(&family))
    }

    /// Opens a component using fonts from the install directory.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(install_dir, build),
            fields(dpi = dpi, width = width, height = height)
        )
    )]
    pub(super) fn open_with_install_fonts(
        dpi: u16,
        width: u32,
        height: u32,
        install_dir: &Path,
        build: impl FnOnce(&str) -> Result<C>,
    ) -> Result<Self> {
        let family = register_install_fonts(install_dir)?;
        Self::open(dpi, width, height, || build(&family))
    }

    /// Draws and presents the first frame as a full-screen update.
    ///
    /// The panel already shows this frame, so its dirty rectangles are cleared
    /// and left out of the next queue.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub(crate) fn present(&self, fb: &mut dyn Framebuffer) -> Result<()> {
        pump();
        self.window.request_redraw();
        if !self.capture_frame() {
            bail!("slint did not render a frame");
        }
        self.blit(fb)?;
        let rect = fb.rect();
        fb.update(&rect, UpdateMode::Full)?;
        self.dirty.borrow_mut().clear();
        Ok(())
    }

    pub(crate) fn component(&self) -> &C {
        &self.component
    }

    /// Physical pixel size captured for this session.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Points the software window at a new framebuffer size.
    ///
    /// The old frame is the previous orientation, so it is dropped. The next
    /// redraw fills the new buffer.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "debug", skip(self)))]
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (self.width == width && self.height == height) {
            return;
        }
        self.width = width;
        self.height = height;
        *self.frame.borrow_mut() = Pixmap::new(width, height, 3);
        self.dirty.borrow_mut().clear();
        self.window
            .set_size(slint::PhysicalSize::new(width, height));
    }

    /// Requests a redraw and captures the frame if Slint rendered one.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(level = "trace", skip_all, ret)
    )]
    pub(crate) fn redraw(&self) -> bool {
        pump();
        self.window.request_redraw();
        self.capture_frame()
    }

    /// Returns the raw changed rectangles accumulated since the previous call.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    pub(crate) fn take_dirty(&self) -> Vec<Rectangle> {
        let rects = std::mem::take(&mut *self.dirty.borrow_mut());
        tracing::trace!(rects = rects.len(), "slint took dirty rectangles");
        rects
    }

    /// Copies the captured frame into the framebuffer without scheduling an update.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    pub(crate) fn blit(&self, fb: &mut dyn Framebuffer) -> Result<()> {
        self.blit_rect(fb, &rect![0, 0, self.width as i32, self.height as i32])
    }

    /// Copies `rect` from the captured frame into the framebuffer.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip(self, fb), fields(%rect)))]
    pub(crate) fn blit_rect(&self, fb: &mut dyn Framebuffer, rect: &Rectangle) -> Result<()> {
        let pixmap = self.frame.borrow();
        let bounds = rect![0, 0, pixmap.width as i32, pixmap.height as i32];
        let Some(rect) = bounds.intersection(rect) else {
            return Ok(());
        };
        if rect.is_empty() {
            return Ok(());
        }
        let started = Instant::now();
        fb.draw_framed_pixmap(&pixmap, &rect, rect.min);
        tracing::trace!(elapsed_us = elapsed_us(started), "slint blit");
        Ok(())
    }

    /// Dispatches a supported device event and captures any resulting frame.
    ///
    /// Slint's clock is advanced before the event. A timer started by this
    /// event would otherwise count from the previous input, and a hold that
    /// came due while the finger was still would fire after the release.
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all, fields(event = ?event), ret))]
    pub(crate) fn handle_device(&self, event: DeviceEvent) -> bool {
        pump();
        match event {
            DeviceEvent::Finger {
                status, position, ..
            } => {
                self.dispatch_pointer(status, position);
                self.tick()
            }
            DeviceEvent::Button { code, status, .. } => {
                if let Some(key) = slint_key(code) {
                    self.dispatch_key(key, status);
                }
                self.tick()
            }
            _ => false,
        }
    }

    fn dispatch_pointer(&self, status: FingerStatus, position: Point) {
        let position = slint::LogicalPosition::new(
            position.x as f32 / self.scale,
            position.y as f32 / self.scale,
        );
        let event = match status {
            FingerStatus::Down => slint::platform::WindowEvent::PointerPressed {
                position,
                button: slint::platform::PointerEventButton::Left,
            },
            FingerStatus::Motion => slint::platform::WindowEvent::PointerMoved { position },
            FingerStatus::Up => slint::platform::WindowEvent::PointerReleased {
                position,
                button: slint::platform::PointerEventButton::Left,
            },
        };
        self.dispatch(event);
    }

    fn dispatch_key(&self, key: slint::platform::Key, status: ButtonStatus) {
        let text = key.into();
        let event = match status {
            ButtonStatus::Pressed => slint::platform::WindowEvent::KeyPressed { text },
            ButtonStatus::Released => slint::platform::WindowEvent::KeyReleased { text },
            ButtonStatus::Repeated => slint::platform::WindowEvent::KeyPressRepeated { text },
        };
        self.dispatch(event);
    }

    fn dispatch(&self, event: slint::platform::WindowEvent) {
        if let Err(error) = self.component.window().dispatch_event_with_result(event) {
            tracing::warn!(error = %error, "slint ignored an input event");
        }
    }

    /// Runs Slint's due timers and captures the frame if anything changed.
    ///
    /// Returns whether Slint captured a new frame.
    pub(crate) fn tick(&self) -> bool {
        pump();
        self.capture_frame()
    }

    /// Asks the hub for an [`Event::SlintTimer`] when Slint's next timer is
    /// due. Call after input and after each tick.
    ///
    /// The wait is a detached task, not a [`Job`](crate::runtime::Job). It
    /// only sleeps, then sends that event. This is the exception to owning
    /// the task: there is nothing to cancel. If the hub has already closed,
    /// the send fails and is dropped. If the Slint screen has already closed,
    /// the current view does not handle [`Event::SlintTimer`], so the main
    /// loop ignores it.
    pub(crate) fn wake_for_timers(&self, hub: &Hub) {
        let Some(delay) = slint::platform::duration_until_next_timer_update() else {
            return;
        };
        let now = Instant::now();
        let due = now + delay;
        if self
            .wake
            .get()
            .is_some_and(|pending| pending > now && pending <= due)
        {
            return;
        }
        self.wake.set(Some(due));
        let hub = hub.clone();
        crate::runtime::current_handle().spawn(async move {
            tokio::time::sleep(delay).await;
            hub.send(Event::SlintTimer.into()).ok();
        });
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(level = "trace", skip_all, ret)
    )]
    fn capture_frame(&self) -> bool {
        let started = Instant::now();
        let mut region = slint::platform::software_renderer::PhysicalRegion::default();
        let mut drew = false;
        let width = self.width;
        let height = self.height;
        self.window.draw_if_needed(|renderer| {
            let mut frame = self.frame.borrow_mut();
            region = renderer.render(frame.data.as_rgb_mut(), width as usize);
            drew = true;
        });
        if !drew {
            tracing::trace!(elapsed_us = elapsed_us(started), "slint skipped a frame");
            return false;
        }
        let rects = dirty_rects(width, height, &region);
        let pixels: u64 = rects
            .iter()
            .map(|rect| u64::from(rect.width()) * u64::from(rect.height()))
            .sum();
        tracing::trace!(
            rects = rects.len(),
            pixels,
            elapsed_us = elapsed_us(started),
            full = pixels >= u64::from(width) * u64::from(height),
            "slint captured a frame"
        );
        self.dirty.borrow_mut().extend(rects);
        true
    }
}

impl<C: ComponentHandle> Drop for Session<C> {
    fn drop(&mut self) {
        let _ = self.component.hide();
    }
}

fn read_app_font(install_dir: &Path) -> Result<Vec<u8>> {
    let path = install_dir.join("fonts").join("NotoSans-Regular.ttf");
    std::fs::read(&path).with_context(|| format!("failed to read slint font {}", path.display()))
}

/// Registers install fonts. CJK files become script fallbacks when present.
/// Noto Sans is the Cyrillic fallback even when those files are absent.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip_all, fields(path = %install_dir.display()))
)]
fn register_install_fonts(install_dir: &Path) -> Result<String> {
    ensure_platform()?;
    let fonts_dir = install_dir.join("fonts");
    let already = LOADED_FONTS.with(|slot| slot.borrow().as_ref() == Some(&fonts_dir));
    if !already {
        let files = font_files(&fonts_dir);
        tracing::debug!(fonts = files.len(), "registering install fonts");
        slint::fontique_011::shared_collection().load_fonts_from_paths(files.iter());
        install_cjk_fallbacks();
        LOADED_FONTS.with(|slot| *slot.borrow_mut() = Some(fonts_dir));
    }
    let mut collection = slint::fontique_011::shared_collection();
    if collection.family_id(APP_FAMILY).is_some() {
        return Ok(APP_FAMILY.to_string());
    }
    register_font(&read_app_font(install_dir)?)
}

fn font_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "ttf" | "otf" | "ttc"))
        })
        .collect();
    files.sort();
    files
}

fn install_cjk_fallbacks() {
    let mut collection = slint::fontique_011::shared_collection();
    let ids: Vec<_> = CJK_FALLBACK_FAMILIES
        .iter()
        .filter_map(|name| collection.family_id(name))
        .collect();
    if !ids.is_empty() {
        collection.append_generic_families(
            slint::fontique_011::fontique::GenericFamily::SansSerif,
            ids.iter().copied(),
        );
        for script in CJK_SCRIPTS {
            collection.append_fallbacks(
                slint::fontique_011::fontique::FallbackKey::new(
                    slint::fontique_011::fontique::Script::from_bytes(*script),
                    None,
                ),
                ids.iter().copied(),
            );
        }
    }
    let Some(app) = collection.family_id(APP_FAMILY) else {
        return;
    };
    collection.append_fallbacks(
        slint::fontique_011::fontique::FallbackKey::new(
            slint::fontique_011::fontique::Script::from_bytes(*b"Cyrl"),
            None,
        ),
        std::iter::once(app),
    );
}

/// Advances Slint timers and animations on the current thread.
fn pump() {
    let started = Instant::now();
    slint::platform::update_timers_and_animations();
    tracing::trace!(elapsed_us = elapsed_us(started), "slint timer pump");
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(bytes), fields(font_family = tracing::field::Empty))
)]
fn register_font(bytes: &[u8]) -> Result<String> {
    ensure_platform()?;
    let blob = slint::fontique_011::fontique::Blob::new(Arc::new(bytes.to_vec()));
    let mut collection = slint::fontique_011::shared_collection();
    let fonts = collection.register_fonts(blob, None);
    let Some((family_id, _)) = fonts.first() else {
        bail!("font contained no families");
    };
    let Some(name) = collection.family_name(*family_id) else {
        bail!("font family has no name");
    };
    let name = name.to_string();
    #[cfg(feature = "tracing")]
    tracing::Span::current().record("font_family", name.as_str());
    tracing::debug!(font_family = %name, "registered slint font");
    Ok(name)
}

fn slint_key(code: ButtonCode) -> Option<slint::platform::Key> {
    match code {
        ButtonCode::Forward => Some(slint::platform::Key::RightArrow),
        ButtonCode::Backward => Some(slint::platform::Key::LeftArrow),
        ButtonCode::Home => Some(slint::platform::Key::Home),
        ButtonCode::Erase => Some(slint::platform::Key::Backspace),
        ButtonCode::Power | ButtonCode::Light | ButtonCode::Highlight | ButtonCode::Raw(_) => None,
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn ensure_platform() -> Result<()> {
    if PLATFORM_READY.with(Cell::get) {
        return Ok(());
    }
    match slint::platform::set_platform(Box::new(SoftwarePlatform)) {
        Ok(()) => tracing::debug!("installed slint software platform"),
        Err(slint::platform::SetPlatformError::AlreadySet) => {}
        Err(error) => return Err(anyhow::anyhow!("failed to install slint platform: {error}")),
    }
    PLATFORM_READY.with(|flag| flag.set(true));
    Ok(())
}

fn dirty_rects(
    width: u32,
    height: u32,
    region: &slint::platform::software_renderer::PhysicalRegion,
) -> Vec<Rectangle> {
    let bounds = rect![0, 0, width as i32, height as i32];
    let mut rects = Vec::new();
    for (origin, size) in region.iter() {
        if size.width == 0 || size.height == 0 {
            continue;
        }
        let rect = rect![
            origin.x,
            origin.y,
            origin
                .x
                .saturating_add(i32::try_from(size.width).unwrap_or(i32::MAX)),
            origin
                .y
                .saturating_add(i32::try_from(size.height).unwrap_or(i32::MAX)),
        ];
        if let Some(rect) = bounds.intersection(&rect)
            && !rect.is_empty()
        {
            rects.push(rect);
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::fontique_011::fontique::{Collection, FallbackKey, FamilyId, Script, SourceKind};

    fn copy_font(dest_dir: &Path, dest_name: &str, src: &Path) {
        std::fs::copy(src, dest_dir.join(dest_name))
            .unwrap_or_else(|error| panic!("copy {} -> {dest_name}: {error}", src.display()));
    }

    fn temp_font_dir(copies: &[(&str, &str)]) -> tempfile::TempDir {
        super::LOADED_FONTS.with(|slot| *slot.borrow_mut() = None);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let tmp = tempfile::tempdir().expect("tempdir");
        let fonts = tmp.path().join("fonts");
        std::fs::create_dir_all(&fonts).expect("fonts dir");
        for (dest_name, rel) in copies {
            copy_font(&fonts, dest_name, &root.join(rel));
        }
        tmp
    }

    fn family_from_dir(collection: &mut Collection, name: &str, fonts_dir: &Path) -> FamilyId {
        let Some(family) = collection.family_by_name(name) else {
            panic!("{name} was not registered");
        };
        assert!(
            family
                .fonts()
                .iter()
                .any(|font| match font.source().kind() {
                    SourceKind::Path(path) => path.starts_with(fonts_dir),
                    SourceKind::Memory(_) => false,
                }),
            "{name} was not loaded from {}",
            fonts_dir.display(),
        );
        family.id()
    }

    #[test]
    fn install_fonts_put_cjk_faces_on_the_fallback_chain() {
        let tmp = temp_font_dir(&[
            ("NotoSans-Regular.ttf", "fonts/NotoSans-Regular.ttf"),
            (
                "NotoSansJP.ttf",
                "thirdparty/google-fonts/ofl/notosansjp/NotoSansJP[wght].ttf",
            ),
            (
                "NotoSansKR.ttf",
                "thirdparty/google-fonts/ofl/notosanskr/NotoSansKR[wght].ttf",
            ),
        ]);
        let fonts = tmp.path().join("fonts");

        let family = register_install_fonts(tmp.path()).expect("register install fonts");
        assert_eq!(family, APP_FAMILY);

        let mut collection = slint::fontique_011::shared_collection();
        let jp = family_from_dir(&mut collection, "Noto Sans JP", &fonts);
        let kr = family_from_dir(&mut collection, "Noto Sans KR", &fonts);
        let hira = FallbackKey::new(Script::from_bytes(*b"Hira"), None);
        assert!(
            collection.fallback_families(hira).any(|id| id == jp),
            "Hiragana fallbacks omit Noto Sans JP"
        );
        let hang = FallbackKey::new(Script::from_bytes(*b"Hang"), None);
        assert!(
            collection.fallback_families(hang).any(|id| id == kr),
            "Hangul fallbacks omit Noto Sans KR"
        );
    }

    #[test]
    fn install_fonts_register_the_keyboard_and_display_faces() {
        let tmp = temp_font_dir(&[
            ("NotoSans-Regular.ttf", "fonts/NotoSans-Regular.ttf"),
            ("NotoSans-Bold.ttf", "fonts/NotoSans-Bold.ttf"),
            ("VarelaRound-Regular.ttf", "fonts/VarelaRound-Regular.ttf"),
            ("Cormorant-Regular.ttf", "fonts/Cormorant-Regular.ttf"),
        ]);
        let fonts = tmp.path().join("fonts");

        let family = register_install_fonts(tmp.path()).expect("register install fonts");
        assert_eq!(family, APP_FAMILY);

        let mut collection = slint::fontique_011::shared_collection();
        family_from_dir(&mut collection, "Varela Round", &fonts);
        family_from_dir(&mut collection, "Cormorant", &fonts);
        let noto = family_from_dir(&mut collection, "Noto Sans", &fonts);
        let cyrl = FallbackKey::new(Script::from_bytes(*b"Cyrl"), None);
        assert!(
            collection.fallback_families(cyrl).any(|id| id == noto),
            "Cyrillic fallbacks omit Noto Sans"
        );
    }
}
