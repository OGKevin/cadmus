//! Full-screen Companion QR panel.
//!
//! Shown when the main-menu Companion switch turns the public listener on.
//! It covers the whole display with a QR code pointing at this device's
//! companion page and the six-digit PIN below it. Any tap closes it via
//! `Event::Close(ViewId::CompanionDialog)`; the switch itself stays the only
//! way to turn the listener off.

use crate::color::{BLACK, WHITE};
use crate::device::{AppContext, DeviceIdentity as _};
use crate::fl;
use crate::font::{DISPLAY_FONT_SIZE, FONT_SIZES, NORMAL_STYLE, font_from_style};
use crate::framebuffer::Pixmap;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::view::image::Image;
use crate::view::label::Label;
use crate::view::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View, ViewId};
use cadmus_api_proto::cadmus::api::shared::v1::Locale;
use cadmus_api_proto::locale_dir;
use qrcode::{EcLevel, QrCode};

/// Quiet zone around the QR modules, in modules (QR specification).
const QUIET_ZONE: u32 = 4;

/// QR error correction level: M survives a scratched screen without growing
/// the module grid enough to matter.
const QR_EC_LEVEL: EcLevel = EcLevel::M;

/// PIN font size; falls back to [`PIN_SMALL_SIZE`] when the code line does
/// not fit the screen width.
const PIN_LARGE_SIZE: u32 = DISPLAY_FONT_SIZE;
const PIN_SMALL_SIZE: u32 = FONT_SIZES[2];

/// URL the QR code encodes: this device's companion page.
///
/// `port` is the port the listener bound. The default HTTP port is left out of
/// the URL. Uses the same locale-dir helper as the public listener, so the
/// panel always advertises a page the listener can serve.
pub(crate) fn companion_url(hostname: &str, port: u16, locale: Locale) -> String {
    let authority = if port == 80 {
        format!("{hostname}.local")
    } else {
        format!("{hostname}.local:{port}")
    };
    format!("http://{authority}/{}/companion/", locale_dir(locale))
}

/// Renders the QR code for `url` as a grayscale pixmap plus its pixel scale.
///
/// The bitmap includes a [`QUIET_ZONE`] quiet zone and upscales the module
/// grid by an integer factor so scanning distance tracks the screen without
/// interpolation blur. `budget` bounds the pixel size per side.
fn qr_pixmap(url: &str, budget: u32) -> Option<(Pixmap, u32)> {
    let code = QrCode::with_error_correction_level(url.as_bytes(), QR_EC_LEVEL).ok()?;
    let modules = code.width() as u32;
    let grid = modules + 2 * QUIET_ZONE;
    let scale = (budget / grid).max(1);
    let side = grid * scale;
    let mut pixmap = Pixmap::new(side, side, 1);
    let data = pixmap.data_mut();
    for y in 0..modules {
        for x in 0..modules {
            if code[(x as usize, y as usize)] == qrcode::Color::Dark {
                let origin = ((QUIET_ZONE + y) * scale * side + (QUIET_ZONE + x) * scale) as usize;
                for row in 0..scale {
                    let start = origin + row as usize * side as usize;
                    data[start..start + scale as usize].fill(BLACK.gray());
                }
            }
        }
    }
    Some((pixmap, scale))
}

pub(crate) struct CompanionDialog {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    qr_side: i32,
    pin_size: u32,
    code_rect: Rectangle,
    code_text: String,
}

impl CompanionDialog {
    /// Builds the panel for the current token and display.
    ///
    /// Returns `None` when the QR code cannot be generated; the caller then
    /// simply skips the panel instead of failing the toggle.
    pub(crate) fn new(context: &mut AppContext, url: &str, pin: &str) -> Option<CompanionDialog> {
        let (width, height) = context.display.dims;
        let code_text = fl!("companion-dialog-code", code = pin).to_string();
        let hint_text = fl!("companion-dialog-hint").to_string();
        let mut dialog = CompanionDialog {
            id: ID_FEEDER.next(),
            rect: rect![0, 0, width as i32, height as i32],
            children: Vec::new(),
            qr_side: 0,
            pin_size: PIN_SMALL_SIZE,
            code_rect: Rectangle::default(),
            code_text,
        };
        dialog.pick_pin_size(context);
        let (pixmap, _scale) = qr_pixmap(url, dialog.qr_budget(width, height, context))?;
        dialog.qr_side = pixmap.width as i32;
        dialog
            .children
            .push(Box::new(Image::new(Rectangle::default(), pixmap)) as Box<dyn View>);
        dialog.children.push(
            Box::new(Label::new(Rectangle::default(), hint_text, Align::Center)) as Box<dyn View>,
        );
        dialog.layout(context);
        Some(dialog)
    }

    /// Uses the large PIN size only when the code line fits the width.
    fn pick_pin_size(&mut self, context: &mut AppContext) {
        let dpi = context.device.dpi();
        let padding = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.em() as i32
        };
        let usable = self.rect.width() as i32 - 2 * padding;
        let fits = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.set_size(PIN_LARGE_SIZE, dpi);
            font.plan(&self.code_text, None, None).width <= usable
        };
        self.pin_size = if fits { PIN_LARGE_SIZE } else { PIN_SMALL_SIZE };
    }

    /// Largest QR pixel size that keeps the vertically centered code and the
    /// PIN block below it on the screen.
    ///
    /// Depends on the PIN size, so it runs after [`Self::pick_pin_size`].
    fn qr_budget(&self, width: u32, height: u32, context: &mut AppContext) -> u32 {
        let dpi = context.device.dpi();
        let below = {
            let (code_line, gap) = {
                let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
                font.set_size(self.pin_size, dpi);
                (font.line_height().max(0) as u32, font.x_heights.0)
            };
            let hint_line = {
                let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
                font.line_height().max(0) as u32
            };
            gap + code_line + hint_line
        };
        (width / 2).min(height.saturating_sub(2 * below))
    }

    /// Places the QR image, PIN line, and hint label from the display
    /// dimensions.
    ///
    /// The QR is centered horizontally and vertically; the PIN and the hint
    /// sit directly below it.
    fn layout(&mut self, context: &mut AppContext) {
        self.pick_pin_size(context);
        let (width, height) = context.display.dims;
        let dpi = context.device.dpi();
        let (code_line, gap) = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.set_size(self.pin_size, dpi);
            (font.line_height(), font.x_heights.0 as i32)
        };
        let hint_line = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.line_height()
        };
        let qr = self.qr_side;
        let top = (height as i32 - qr) / 2;
        let left = (width as i32 - qr) / 2;
        *self.children[0].rect_mut() = rect![left, top, left + qr, top + qr];
        self.code_rect = rect![0, top + qr + gap, width as i32, top + qr + gap + code_line];
        *self.children[1].rect_mut() = rect![
            0,
            self.code_rect.max.y,
            width as i32,
            self.code_rect.max.y + hint_line
        ];
    }
}

#[async_trait::async_trait(?Send)]
impl View for CompanionDialog {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, _bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    async fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(_)) => {
                hub.send((Event::Close(ViewId::CompanionDialog)).into())
                    .ok();
                true
            }
            Event::Gesture(..) => true,
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context, _rect), fields(rect = ?_rect)))]
    fn render(&self, context: &mut AppContext, _rect: Rectangle) {
        let (fb, fonts, dpi) = context.framebuffer_and_fonts();
        fb.draw_rectangle(&self.rect, WHITE);

        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        font.set_size(self.pin_size, dpi);
        let x_height = font.x_heights.0 as i32;
        let plan = font.plan(&self.code_text, None, None);
        let dx = (self.rect.width() as i32 - plan.width) / 2;
        let dy = (self.code_rect.height() as i32 - x_height) / 2;
        let pt = pt!(self.rect.min.x + dx, self.code_rect.max.y - dy);
        font.render(fb, BLACK, &plan, pt);
    }

    fn resize(
        &mut self,
        rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        self.rect = rect;
        self.layout(context);
        for child in &mut self.children {
            let child_rect = *child.rect();
            child.resize(child_rect, hub, rq, context);
        }
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

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::CompanionDialog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use std::collections::VecDeque;

    #[test]
    fn companion_url_uses_mdns_hostname_and_listener_locale_path() {
        assert_eq!(
            companion_url("cadmus", 80, Locale::LOCALE_EN),
            "http://cadmus.local/en/companion/"
        );
        assert_eq!(
            companion_url("cadmus-2", 80, Locale::LOCALE_FR),
            "http://cadmus-2.local/fr/companion/"
        );
        let unspecified = companion_url("cadmus", 80, Locale::LOCALE_UNSPECIFIED);
        assert!(
            unspecified.starts_with("http://cadmus.local/en/companion/"),
            "unspecified locale falls back to the default site dir: {unspecified}"
        );
        let (_host, path) = unspecified
            .split_once(".local/")
            .expect("hostname separator");
        assert!(
            !path.starts_with('/'),
            "no empty locale dir in the path: {path}"
        );
    }

    #[test]
    fn companion_url_names_the_bound_port_when_not_http_default() {
        assert_eq!(
            companion_url("cadmus", 40123, Locale::LOCALE_EN),
            "http://cadmus.local:40123/en/companion/"
        );
    }

    #[test]
    fn qr_pixmap_is_square_with_quiet_zone_and_integer_scale() {
        let (pixmap, scale) =
            qr_pixmap("http://cadmus.local/en/companion/", 232).expect("url encodes");
        assert_eq!(pixmap.width, pixmap.height);
        assert_eq!(pixmap.samples, 1);
        assert!(
            pixmap.width % scale == 0,
            "side must be grid × integer scale"
        );
        assert!(
            pixmap.data().contains(&BLACK.gray()),
            "code has dark modules"
        );

        let side = pixmap.width as usize;
        let quiet = (QUIET_ZONE * scale) as usize;
        let data = pixmap.data();
        for y in 0..quiet {
            for x in 0..side {
                assert_eq!(data[y * side + x], WHITE.gray(), "top quiet zone");
                assert_eq!(
                    data[(side - 1 - y) * side + x],
                    WHITE.gray(),
                    "bottom quiet zone"
                );
            }
        }
        for y in 0..side {
            for x in 0..quiet {
                assert_eq!(data[y * side + x], WHITE.gray(), "left quiet zone");
                assert_eq!(
                    data[y * side + side - 1 - x],
                    WHITE.gray(),
                    "right quiet zone"
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn any_tap_sends_close() {
        let mut context = create_test_context().await;
        let (hub, mut receiver) = crate::view::hub_channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut dialog =
            CompanionDialog::new(&mut context, "http://cadmus.local/en/companion/", "123456")
                .expect("dialog builds");

        let qr_center = dialog.children[0].rect().center();
        let (_, height) = context.display.dims;
        let corner = Point::new(10, height as i32 - 10);
        for point in [qr_center, corner] {
            let event = Event::Gesture(GestureEvent::Tap(point));
            assert!(
                dialog
                    .handle_event(&event, &hub, &mut bus, &mut rq, &mut context)
                    .await
            );
            let message = receiver.try_recv().expect("close event on hub");
            assert!(matches!(
                message.event,
                Event::Close(ViewId::CompanionDialog)
            ));
        }
        assert!(bus.is_empty(), "taps go to the hub, not the parent bus");
    }
}
