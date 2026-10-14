use std::time::Instant;

use anyhow::Context as _;

use super::slint;
use super::slint::generated::StartupWindow;
use crate::device::{AppDevice, DeviceHardware as _};

fn new_startup_window() -> anyhow::Result<StartupWindow> {
    StartupWindow::new().context("failed to create slint startup window")
}

/// Presents the startup frame, then drops the Slint session.
///
/// The panel keeps the image. The session holds a full-screen frame, so it
/// is released before the rest of startup.
#[cfg_attr(feature = "tracing", tracing::instrument(skip(device)))]
pub fn open(device: &mut AppDevice) -> anyhow::Result<()> {
    let started = Instant::now();
    let ui = slint::open(device, |_| new_startup_window())?;
    {
        let fb = device.framebuffer_mut();
        ui.present(fb)?;
    }
    tracing::info!(
        renderer = "slint",
        duration_ms = elapsed_ms(started),
        "startup screen presented"
    );
    Ok(())
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Error;

    use crate::color::Color;
    use crate::framebuffer::{Framebuffer, Pixmap, UpdateMode};
    use crate::geom::Rectangle;

    fn startup_window(family: &str) -> anyhow::Result<super::StartupWindow> {
        let ui = super::new_startup_window()?;
        ui.set_font_family(family.into());
        Ok(ui)
    }

    struct RecordingFramebuffer {
        pixmap: Pixmap,
        updates: Vec<(Rectangle, UpdateMode)>,
    }

    impl RecordingFramebuffer {
        fn new(width: u32, height: u32) -> Self {
            Self {
                pixmap: Pixmap::new(width, height, 3),
                updates: Vec::new(),
            }
        }
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

    fn font_bytes() -> Vec<u8> {
        let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts/NotoSans-Regular.ttf");
        std::fs::read(&font).unwrap_or_else(|error| panic!("read {}: {error}", font.display()))
    }

    #[test]
    fn startup_frame_shows_text_and_requests_full_refresh() {
        let font = font_bytes();
        for (width, height, dpi) in [(600, 800, 167_u16), (1072, 1448, 300)] {
            let mut fb = RecordingFramebuffer::new(width, height);
            let ui = crate::view::slint::open_with_font(dpi, width, height, &font, startup_window)
                .expect("startup session");
            ui.present(&mut fb).expect("startup frame");
            let rect = fb.rect();
            assert_eq!((fb.width(), fb.height()), (width, height));
            assert_eq!(fb.updates.as_slice(), &[(rect, UpdateMode::Full)]);
            for (x, y) in [
                (0, 0),
                (width - 1, 0),
                (0, height - 1),
                (width - 1, height - 1),
            ] {
                assert!(
                    fb.pixmap.get_pixel(x, y).gray() >= 250,
                    "expected a white background at {x},{y} for {width}x{height}"
                );
            }
            assert!(
                fb.pixmap.data().iter().any(|sample| *sample < 80),
                "expected visible startup text at {width}x{height}"
            );
        }
    }

    #[test]
    fn startup_message_follows_the_bundled_french_translation() {
        let font = font_bytes();
        let ui = crate::view::slint::open_with_font(167, 600, 800, &font, startup_window)
            .expect("startup session");
        slint::select_bundled_translation("en").expect("english catalog");
        assert_eq!(ui.component().get_message().as_str(), "Cadmus starting up…");
        slint::select_bundled_translation("fr").expect("french catalog");
        assert_eq!(ui.component().get_message().as_str(), "Cadmus démarre…");
    }
}
