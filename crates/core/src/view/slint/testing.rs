//! Shared test helpers for Slint screens.

use anyhow::Error;

use crate::color::Color;
use crate::device::AppContext;
use crate::framebuffer::{Framebuffer, Pixmap, UpdateMode};
use crate::geom::Rectangle;
use crate::view::slint::{SlintHost, SlintScreen};

/// Writes the UI fonts into the device install directory.
///
/// Settings and OTA text use Noto Sans Regular and Bold. The on-screen
/// keyboard uses Varela Round. The install directory is one fixed path
/// shared by every test process, so each write is atomic: a concurrent
/// `register_install_fonts` in another process must never read a half-written
/// font and fail to open its session.
pub(crate) async fn install_ui_font(context: &AppContext) {
    use std::path::Path;

    use crate::device::DevicePaths as _;

    let fonts_dir = context.device.install_dir().join("fonts");
    tokio::fs::create_dir_all(&fonts_dir)
        .await
        .expect("create fonts dir");
    for file_name in [
        "NotoSans-Regular.ttf",
        "NotoSans-Bold.ttf",
        "VarelaRound-Regular.ttf",
    ] {
        let font = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fonts")
            .join(file_name);
        let bytes = tokio::fs::read(&font)
            .await
            .unwrap_or_else(|error| panic!("read {}: {error}", font.display()));
        let dest = fonts_dir.join(file_name);
        tokio::task::spawn_blocking(move || {
            let tmp = tempfile::Builder::new()
                .prefix(".ui-font-")
                .tempfile_in(dest.parent().expect("fonts dir"))
                .expect("temp font file");
            std::fs::write(tmp.path(), &bytes).expect("write font");
            tmp.persist(&dest).expect("persist font");
        })
        .await
        .expect("install font");
    }
}

/// Framebuffer that records update rectangles for pixel tests.
pub(crate) struct RecordingFramebuffer {
    pub(crate) pixmap: Pixmap,
    pub(crate) updates: Vec<(Rectangle, UpdateMode)>,
}
impl RecordingFramebuffer {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        Self {
            pixmap: Pixmap::new(width, height, 3),
            updates: Vec::new(),
        }
    }
}

/// Opens a screen on a test device and returns the host with its context.
///
/// The fonts are installed first, so the window opens with the app font.
pub(crate) async fn open_screen<S: SlintScreen>(screen: S) -> (AppContext, SlintHost<S>) {
    use crate::context::test_helpers::create_test_context;
    use crate::device::DeviceHardware as _;

    let mut context = create_test_context().await;
    install_ui_font(&context).await;
    let rect = context.device.framebuffer().rect();
    let mut rq = crate::view::RenderQueue::new();
    let host = SlintHost::mount(rect, &mut rq, &mut context, screen);
    (context, host)
}

impl Framebuffer for RecordingFramebuffer {
    fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        self.pixmap.set_pixel(x, y, color);
    }

    fn set_blended_pixel(&mut self, x: u32, y: u32, color: Color, alpha: f32) {
        self.pixmap.set_blended_pixel(x, y, color, alpha);
    }

    fn invert_region(&mut self, rect: &Rectangle) {
        self.pixmap.invert_region(rect);
    }

    fn shift_region(&mut self, rect: &Rectangle, drift: u8) {
        self.pixmap.shift_region(rect, drift);
    }

    fn update(&mut self, rect: &Rectangle, mode: UpdateMode) -> Result<u32, Error> {
        self.updates.push((*rect, mode));
        Ok(1)
    }

    fn wait(&self, _token: u32) -> Result<i32, Error> {
        Ok(1)
    }

    fn save(&self, _path: &str) -> Result<(), Error> {
        Ok(())
    }

    fn set_rotation(&mut self, _n: i8) -> Result<(u32, u32), Error> {
        Ok(self.pixmap.dims())
    }

    fn set_monochrome(&mut self, enable: bool) {
        self.pixmap.set_monochrome(enable);
    }

    fn set_dithered(&mut self, enable: bool) {
        self.pixmap.set_dithered(enable);
    }

    fn set_inverted(&mut self, enable: bool) {
        self.pixmap.set_inverted(enable);
    }

    fn monochrome(&self) -> bool {
        self.pixmap.monochrome()
    }

    fn dithered(&self) -> bool {
        self.pixmap.dithered()
    }

    fn inverted(&self) -> bool {
        self.pixmap.inverted()
    }

    fn width(&self) -> u32 {
        self.pixmap.width()
    }

    fn height(&self) -> u32 {
        self.pixmap.height()
    }

    fn rotation(&self) -> i8 {
        self.pixmap.rotation()
    }
}
