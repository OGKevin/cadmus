//! Full-screen Companion view.
//!
//! Opened from the main-menu Companion entry, which starts the public listener.
//! It covers the whole display with a QR code pointing at this device's
//! companion page, the six-digit PIN below it, and the page's web address by
//! name and by number. The Stop Companion button closes it via
//! `Event::Close(ViewId::Companion)`, which also stops the listener; other
//! taps do nothing.

use crate::color::{BLACK, WHITE};
use crate::device::{AppContext, DeviceIdentity as _};
use crate::fl;
use crate::font::{DISPLAY_FONT_SIZE, FONT_SIZES, NORMAL_STYLE, font_from_style};
use crate::framebuffer::Pixmap;
use crate::geom::Rectangle;
use crate::view::button::Button;
use crate::view::image::Image;
use crate::view::label::Label;
use crate::view::{Align, Bus, Event, Hub, ID_FEEDER, Id, RenderQueue, View, ViewId};
use cadmus_api_proto::cadmus::api::shared::v1::Locale;
use cadmus_api_proto::locale_dir;
use qrcode::{EcLevel, QrCode};
use std::net::{IpAddr, SocketAddr};

/// Quiet zone around the QR modules, in modules (QR specification).
const QUIET_ZONE: u32 = 4;

/// QR error correction level: M survives a scratched screen without growing
/// the module grid enough to matter.
const QR_EC_LEVEL: EcLevel = EcLevel::M;

/// PIN font size; falls back to [`PIN_SMALL_SIZE`] when the code line does
/// not fit the screen width.
const PIN_LARGE_SIZE: u32 = DISPLAY_FONT_SIZE;
const PIN_SMALL_SIZE: u32 = FONT_SIZES[2];

/// Address font size; falls back to [`URL_SMALL_SIZE`] when either address
/// does not fit the screen width.
const URL_LARGE_SIZE: u32 = FONT_SIZES[1];
const URL_SMALL_SIZE: u32 = FONT_SIZES[0];

/// URL the QR code encodes: this device's companion page.
///
/// `port` is the port the listener bound. The default HTTP port is left out of
/// the URL. Uses the same locale-dir helper as the public listener, so the
/// view always advertises a page the listener can serve.
pub(crate) fn companion_url(hostname: &str, port: u16, locale: Locale) -> String {
    let authority = if port == 80 {
        format!("{hostname}.local")
    } else {
        format!("{hostname}.local:{port}")
    };
    format!("http://{authority}/{}/companion/", locale_dir(locale))
}

/// Number-based companion page URL for `ip` and the bound `port`, shown next
/// to the name URL. The default HTTP port is left out, and IPv6 hosts are
/// bracketed.
pub(crate) fn companion_ip_url(ip: IpAddr, port: u16, locale: Locale) -> String {
    let authority = if port == 80 {
        match ip {
            IpAddr::V4(_) => ip.to_string(),
            IpAddr::V6(_) => format!("[{ip}]"),
        }
    } else {
        SocketAddr::new(ip, port).to_string()
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

/// Size of the Stop Companion button, taken from the normal font: padding is
/// one em and height four x-heights, as the dialog buttons.
struct StopMetrics {
    padding: i32,
    height: i32,
    width: i32,
}

impl StopMetrics {
    /// Vertical space reserved at the bottom: the button plus padding above and below.
    fn zone(&self) -> i32 {
        self.height + 2 * self.padding
    }
}

fn stop_metrics(context: &mut AppContext) -> StopMetrics {
    let dpi = context.device.dpi();
    let label = fl!("companion-view-stop").to_string();
    let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
    let padding = font.em() as i32;
    StopMetrics {
        padding,
        height: 4 * font.x_heights.0 as i32,
        width: font.plan(&label, None, None).width + 2 * padding,
    }
}

/// Companion view: the QR code, the PIN line, the hint, the name and number
/// addresses, the line saying whether Companion keeps the device awake, and
/// the Stop Companion button.
///
/// `children` holds the QR image (0), the hint label (1), the awake label (2),
/// and the Stop Companion button (3). The address lines are not children:
/// `Label` draws at a fixed size and the addresses may need a smaller one, so
/// `render` draws them from `url_texts` into `url_rects`.
pub(crate) struct CompanionView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    qr_side: i32,
    pin_size: u32,
    code_rect: Rectangle,
    code_text: String,
    url_texts: Vec<String>,
    url_size: u32,
    url_rects: Vec<Rectangle>,
}

impl CompanionView {
    /// Builds the view for the current PIN and display.
    ///
    /// `url` is the name address the QR code encodes; `ip_url`, when present,
    /// is the number address shown below it. `awake` tells the view whether
    /// Companion holds the Sleep lease: when it does, the awake label says the
    /// device stays awake, and otherwise that a low battery may let it sleep.
    /// Returns `None` when the QR code cannot be generated; the caller then
    /// simply skips the view instead of failing the start.
    pub(crate) fn new(
        context: &mut AppContext,
        url: &str,
        ip_url: Option<&str>,
        pin: &str,
        awake: bool,
    ) -> Option<CompanionView> {
        let (width, height) = context.display.dims;
        let code_text = fl!("companion-view-code", code = pin).to_string();
        let hint_text = fl!("companion-view-hint").to_string();
        let awake_text = if awake {
            fl!("companion-view-awake").to_string()
        } else {
            fl!("companion-view-low-battery").to_string()
        };
        let mut url_texts = vec![url.to_string()];
        url_texts.extend(ip_url.map(str::to_string));
        let mut dialog = CompanionView {
            id: ID_FEEDER.next(),
            rect: rect![0, 0, width as i32, height as i32],
            children: Vec::new(),
            qr_side: 0,
            pin_size: PIN_SMALL_SIZE,
            code_rect: Rectangle::default(),
            code_text,
            url_texts,
            url_size: URL_SMALL_SIZE,
            url_rects: Vec::new(),
        };
        dialog.pick_pin_size(context);
        dialog.pick_url_size(context);
        let (pixmap, _scale) = qr_pixmap(url, dialog.qr_budget(width, height, context))?;
        dialog.qr_side = pixmap.width as i32;
        dialog
            .children
            .push(Box::new(Image::new(Rectangle::default(), pixmap)) as Box<dyn View>);
        dialog.children.push(
            Box::new(Label::new(Rectangle::default(), hint_text, Align::Center)) as Box<dyn View>,
        );
        dialog.children.push(
            Box::new(Label::new(Rectangle::default(), awake_text, Align::Center)) as Box<dyn View>,
        );
        dialog.children.push(Box::new(Button::new(
            Rectangle::default(),
            Event::Close(ViewId::Companion),
            fl!("companion-view-stop").to_string(),
        )) as Box<dyn View>);
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

    /// Uses the large address size only when every address fits the width
    /// beside the padding; otherwise every address uses the small size.
    fn pick_url_size(&mut self, context: &mut AppContext) {
        let dpi = context.device.dpi();
        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let padding = font.em() as i32;
        let usable = self.rect.width() as i32 - 2 * padding;
        font.set_size(URL_LARGE_SIZE, dpi);
        let fits = self
            .url_texts
            .iter()
            .all(|url| font.plan(url, None, None).width <= usable);
        self.url_size = if fits { URL_LARGE_SIZE } else { URL_SMALL_SIZE };
    }

    /// Largest QR pixel size that keeps the code, the PIN, the hint, the
    /// address lines, the awake line, and the Stop Companion button zone on
    /// the screen.
    ///
    /// Depends on the PIN and address sizes, so it runs after
    /// [`Self::pick_pin_size`] and [`Self::pick_url_size`].
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
            let url_line = {
                let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
                font.set_size(self.url_size, dpi);
                font.line_height().max(0) as u32
            };
            gap + code_line + 2 * hint_line + self.url_texts.len() as u32 * url_line
        };
        let zone = stop_metrics(context).zone() as u32;
        (width / 2).min(height.saturating_sub(zone + 2 * below))
    }

    /// Places the QR image, PIN line, hint label, awake label, address lines,
    /// and the Stop Companion button from the display dimensions.
    ///
    /// The QR block is centered in the area above the button zone; the PIN,
    /// the hint, and the address lines sit directly below it, and the button
    /// is centered at the bottom.
    fn layout(&mut self, context: &mut AppContext) {
        self.pick_pin_size(context);
        self.pick_url_size(context);
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
        let url_line = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.set_size(self.url_size, dpi);
            font.line_height()
        };
        let stop = stop_metrics(context);
        let qr = self.qr_side;
        let usable_height = height as i32 - stop.zone();
        let top = (usable_height - qr) / 2;
        let left = (width as i32 - qr) / 2;
        *self.children[0].rect_mut() = rect![left, top, left + qr, top + qr];
        self.code_rect = rect![0, top + qr + gap, width as i32, top + qr + gap + code_line];
        *self.children[1].rect_mut() = rect![
            0,
            self.code_rect.max.y,
            width as i32,
            self.code_rect.max.y + hint_line
        ];
        let mut line_top = self.children[1].rect().max.y;
        self.url_rects.clear();
        for _ in 0..self.url_texts.len() {
            self.url_rects
                .push(rect![0, line_top, width as i32, line_top + url_line]);
            line_top += url_line;
        }
        *self.children[2].rect_mut() = rect![0, line_top, width as i32, line_top + hint_line];
        let button_left = (width as i32 - stop.width) / 2;
        let button_bottom = height as i32 - stop.padding;
        *self.children[3].rect_mut() = rect![
            button_left,
            button_bottom - stop.height,
            button_left + stop.width,
            button_bottom
        ];
    }
}

#[async_trait::async_trait(?Send)]
impl View for CompanionView {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _hub, _bus, _rq, _context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    async fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        _bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut AppContext,
    ) -> bool {
        matches!(*evt, Event::Gesture(..))
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

        font.set_size(self.url_size, dpi);
        let url_x_height = font.x_heights.0 as i32;
        for (url, url_rect) in self.url_texts.iter().zip(&self.url_rects) {
            let plan = font.plan(url, None, None);
            let dx = (url_rect.width() as i32 - plan.width) / 2;
            let dy = (url_rect.height() as i32 - url_x_height) / 2;
            let pt = pt!(url_rect.min.x + dx, url_rect.max.y - dy);
            font.render(fb, BLACK, &plan, pt);
        }
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
        Some(ViewId::Companion)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::geom::Point;
    use crate::gesture::GestureEvent;
    use std::collections::VecDeque;
    use std::net::{Ipv4Addr, Ipv6Addr};

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
    fn companion_ip_url_omits_the_default_port_and_keeps_others() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 128));
        assert_eq!(
            companion_ip_url(ip, 80, Locale::LOCALE_EN),
            "http://192.168.1.128/en/companion/"
        );
        assert_eq!(
            companion_ip_url(ip, 40123, Locale::LOCALE_EN),
            "http://192.168.1.128:40123/en/companion/"
        );
    }

    #[test]
    fn companion_ip_url_brackets_ipv6_hosts() {
        let ip = IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().expect("valid address"));
        assert_eq!(
            companion_ip_url(ip, 80, Locale::LOCALE_EN),
            "http://[2001:db8::1]/en/companion/"
        );
        assert_eq!(
            companion_ip_url(ip, 40123, Locale::LOCALE_EN),
            "http://[2001:db8::1]:40123/en/companion/"
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
    async fn tap_outside_the_stop_button_is_swallowed() {
        let mut context = create_test_context().await;
        let (hub, mut receiver) = crate::view::hub_channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut view = CompanionView::new(
            &mut context,
            "http://cadmus.local/en/companion/",
            None,
            "123456",
            true,
        )
        .expect("view builds");

        for point in [view.children[0].rect().center(), Point::new(10, 10)] {
            let event = Event::Gesture(GestureEvent::Tap(point));
            assert!(
                crate::view::handle_event(&mut view, &event, &hub, &mut bus, &mut rq, &mut context)
                    .await
            );
        }
        assert!(bus.is_empty(), "no event reaches the parent bus");
        assert!(receiver.try_recv().is_err(), "nothing reaches the hub");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_button_tap_asks_to_close_once() {
        let mut context = create_test_context().await;
        let (hub, mut receiver) = crate::view::hub_channel();
        let mut bus = VecDeque::new();
        let mut rq = RenderQueue::new();
        let mut view = CompanionView::new(
            &mut context,
            "http://cadmus.local/en/companion/",
            None,
            "123456",
            true,
        )
        .expect("view builds");

        let button = view.children[3].rect().center();
        let event = Event::Gesture(GestureEvent::Tap(button));
        assert!(
            crate::view::handle_event(&mut view, &event, &hub, &mut bus, &mut rq, &mut context)
                .await
        );
        assert!(receiver.try_recv().is_err(), "nothing reaches the hub");
        assert!(matches!(
            bus.pop_front(),
            Some(Event::Close(ViewId::Companion))
        ));
        assert!(bus.is_empty(), "the close request is queued exactly once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn view_without_number_address_shows_only_the_name_line() {
        let mut context = create_test_context().await;
        let view = CompanionView::new(
            &mut context,
            "http://cadmus.local/en/companion/",
            None,
            "123456",
            true,
        )
        .expect("view builds");
        assert_eq!(view.url_texts.len(), 1);
        assert_eq!(view.url_rects.len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn long_addresses_fit_the_display_above_the_stop_button() {
        let mut context = create_test_context().await;
        let (width, _) = context.display.dims;
        let dpi = context.device.dpi();
        let name_url = companion_url("cadmus-2", 65535, Locale::LOCALE_EN);
        let ip_url = companion_ip_url(
            IpAddr::V4(Ipv4Addr::new(192, 168, 100, 200)),
            65535,
            Locale::LOCALE_EN,
        );
        let view = CompanionView::new(&mut context, &name_url, Some(&ip_url), "123456", true)
            .expect("view builds");
        assert_eq!(view.url_rects.len(), 2);

        let button_top = view.children[3].rect().min.y;
        let padding = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.em() as i32
        };
        for (url, rect) in view.url_texts.iter().zip(&view.url_rects) {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.set_size(view.url_size, dpi);
            let text_width = font.plan(url, None, None).width;
            assert!(
                text_width <= width as i32 - 2 * padding,
                "{url} fits the width at the chosen size: {text_width}"
            );
            assert!(rect.max.x <= width as i32, "line stays on the display");
            assert!(rect.max.y <= button_top, "line clears the Stop button");
        }
    }

    fn assert_awake_line(context: &mut AppContext, view: &CompanionView, expected: &str) {
        let dpi = context.device.dpi();
        let awake = view.children[2]
            .downcast_ref::<Label>()
            .expect("the awake line is a label");
        assert_eq!(awake.text(), expected);
        let padding = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.em() as i32
        };
        let text_width = {
            let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
            font.plan(awake.text(), None, None).width
        };
        assert!(
            text_width <= awake.rect().width() as i32 - padding,
            "a Label truncates beyond its width less one em: {text_width}"
        );
        let last_url = view.url_rects.last().expect("address lines");
        assert!(awake.rect().min.y >= last_url.max.y, "below the addresses");
        assert!(
            awake.rect().max.y <= view.children[3].rect().min.y,
            "clears the Stop button"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn awake_line_fits_the_narrowest_display_between_the_addresses_and_the_button() {
        let mut context = create_test_context().await;
        let (width, _) = context.display.dims;
        assert_eq!(width, 600, "600 px is the narrowest Kobo display");
        let name_url = companion_url("cadmus-2", 65535, Locale::LOCALE_EN);
        let ip_url = companion_ip_url(
            IpAddr::V4(Ipv4Addr::new(192, 168, 100, 200)),
            65535,
            Locale::LOCALE_EN,
        );

        for (awake, expected) in [
            (true, fl!("companion-view-awake").to_string()),
            (false, fl!("companion-view-low-battery").to_string()),
        ] {
            let view = CompanionView::new(&mut context, &name_url, Some(&ip_url), "123456", awake)
                .expect("view builds");
            assert_awake_line(&mut context, &view, &expected);
        }
    }
}
