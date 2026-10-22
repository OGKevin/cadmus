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

    use crate::framebuffer::{Framebuffer as _, UpdateMode};
    use crate::view::slint::testing::RecordingFramebuffer;

    fn startup_window(_family: &str) -> anyhow::Result<super::StartupWindow> {
        super::new_startup_window()
    }

    /// The face the startup sentence uses.
    fn font_bytes() -> Vec<u8> {
        let font = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fonts/Cormorant-Regular.ttf");
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
