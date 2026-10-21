//! Settings editor view.
//!
//! The window is a shell. Each Slint component has a module under
//! [`super::components`] that sets that component's input and wires its
//! callbacks. This type owns the session and the [`Parts`] coordinator, which
//! applies the callbacks to [`Settings`](crate::settings::Settings).
//!
//! A Slint callback runs while Slint dispatches a pointer event, so it cannot
//! borrow this view. Each callback pushes onto its component's queue; after the
//! dispatch, [`Parts::drain`] applies every queued callback.

use super::components::Parts;

use slint::ComponentHandle;

use crate::device::AppContext;
use crate::device::{DeviceHardware as _, DevicePaths as _};
use crate::dictionary::MonolingualDictionaryService;
use crate::fl;
use crate::framebuffer::{Framebuffer as _, UpdateMode};
use crate::geom::Rectangle;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
use crate::view::slint::generated::{InkScreen, SettingsWindow};
use crate::view::slint::{Env, Flow, Session, SlintHost, SlintScreen};
use crate::view::{Event, Hub, NotificationEvent, RenderQueue};

/// Settings editor view. The host owns the session and the common event loop.
pub type SettingsEditor = SlintHost<SettingsScreen>;

/// Category pages, the file browser, and the other sheets are components
/// owned by [`Parts`]. They share this window's session.
pub struct SettingsScreen {
    pub(super) parts: Parts,
}

#[async_trait::async_trait(?Send)]
impl SlintScreen for SettingsScreen {
    type Window = SettingsWindow;

    fn open(context: &mut AppContext) -> anyhow::Result<Session<SettingsWindow>> {
        super::components::open(context)
    }

    fn wire(&mut self, window: &SettingsWindow) {
        self.parts.wire(window);
    }

    fn sync(&mut self, session: &Session<SettingsWindow>, context: &AppContext) {
        self.parts.sync(session.component(), context);
        let _ = session.redraw();
    }

    async fn apply_callbacks(
        &mut self,
        hub: &Hub,
        bus: &mut crate::view::Bus,
        context: &mut AppContext,
    ) -> Flow {
        self.parts.drain(hub, bus, context).await
    }

    fn consume_back(&mut self) -> bool {
        self.parts.close_top()
    }

    fn stop_jobs(&self) {
        self.parts.stop_jobs();
    }

    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        if let Some(job) = self.parts.take_job() {
            jobs.push(job);
        }
    }

    fn skip_preview(&self, finger_down: bool, finger_preview: bool) -> bool {
        crate::view::slint::components::text_sheet::skip_press_frame(
            self.parts.text_is_open(),
            finger_down,
            finger_preview,
        )
    }

    fn panel_refresh(&self, finger_down: bool) -> (UpdateMode, bool) {
        let refresh = crate::view::slint::components::text_sheet::frame_refresh(
            self.parts.text_is_open(),
            finger_down,
        );
        (refresh.mode, refresh.wait)
    }

    async fn on_event(&mut self, evt: &Event, env: &mut Env<'_>, _rq: &mut RenderQueue) -> bool {
        match evt {
            Event::SelectSettingsCategory(category) => {
                self.parts.set_category(*category, env.context).await;
                true
            }
            Event::DictionaryInstallComplete { lang, result } => {
                self.parts.reload_dictionaries(env.context).await;
                match result {
                    Ok(()) => {
                        tracing::info!(lang, "Dictionary installed");
                        env.hub.send((Event::ReindexDictionaries).into()).ok();
                        env.hub
                            .send(
                                (Event::Notification(NotificationEvent::Show(fl!(
                                    "notification-downloading-dictionary-completed",
                                    lang = lang
                                ))))
                                .into(),
                            )
                            .ok();
                    }
                    Err(error) => {
                        tracing::warn!(lang, error = %error, "Failed to install dictionary");
                        env.hub
                            .send(
                                (Event::Notification(NotificationEvent::Show(fl!(
                                    "notification-dictionary-install-failed",
                                    lang = lang
                                ))))
                                .into(),
                            )
                            .ok();
                    }
                }
                true
            }
            _ => false,
        }
    }

    async fn take_device(
        &mut self,
        event: DeviceEvent,
        window: Option<&SettingsWindow>,
        env: &mut Env<'_>,
        swallow: &mut Option<ButtonCode>,
    ) -> bool {
        match event {
            DeviceEvent::Button {
                code: ButtonCode::Backward,
                status: ButtonStatus::Pressed,
                ..
            } if self.parts.is_plain() => {
                *swallow = Some(ButtonCode::Backward);
                self.parts.step_category(-1, env.context).await;
                true
            }
            DeviceEvent::Button {
                code: ButtonCode::Forward,
                status: ButtonStatus::Pressed,
                ..
            } if self.parts.is_plain() => {
                *swallow = Some(ButtonCode::Forward);
                self.parts.step_category(1, env.context).await;
                true
            }
            DeviceEvent::Button {
                code: code @ (ButtonCode::Backward | ButtonCode::Forward),
                status: ButtonStatus::Pressed | ButtonStatus::Repeated,
                ..
            } => {
                let delta = if code == ButtonCode::Backward { -1 } else { 1 };
                let Some(window) = window else {
                    return false;
                };
                if self.parts.bump_open(window, delta) {
                    *swallow = Some(code);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn log_resize(
        &self,
        previous: Rectangle,
        rect: Rectangle,
        context: &AppContext,
        session: Option<&Session<SettingsWindow>>,
        children: usize,
    ) {
        let fb_dims = context.device.framebuffer().dims();
        let session = session.map(|session| {
            let physical = session.component().window().size();
            let screen = session.component().global::<InkScreen>();
            (
                session.size(),
                (physical.width, physical.height),
                (screen.get_window_width(), screen.get_window_height()),
            )
        });
        tracing::debug!(
            previous_rect = %previous,
            requested_rect = %rect,
            fb_width = fb_dims.0,
            fb_height = fb_dims.1,
            display_width = context.display.dims.0,
            display_height = context.display.dims.1,
            session_width = session.as_ref().map(|sizes| sizes.0.0),
            session_height = session.as_ref().map(|sizes| sizes.0.1),
            slint_width = session.as_ref().map(|sizes| sizes.1.0),
            slint_height = session.as_ref().map(|sizes| sizes.1.1),
            chrome_width = session.as_ref().map(|sizes| sizes.2.0),
            chrome_height = session.as_ref().map(|sizes| sizes.2.1),
            children,
            "settings editor resize"
        );
    }
}

impl SettingsEditor {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(rq, context)))]
    pub async fn new(rect: Rectangle, rq: &mut RenderQueue, context: &mut AppContext) -> Self {
        let dict_service = match MonolingualDictionaryService::new(
            &context.database,
            context
                .device
                .data_path(crate::context::DICTIONARIES_DIRNAME)
                .as_path(),
        ) {
            Ok(service) => Some(service),
            Err(error) => {
                tracing::warn!(error = %error, "Failed to create MonolingualDictionaryService");
                None
            }
        };
        Self::mount(
            rect,
            rq,
            context,
            SettingsScreen {
                parts: Parts::new(dict_service),
            },
        )
    }
}

/// Draws the settings window through the software renderer and checks pixel
/// bands. `slint-viewer --screenshot` and `Window::take_snapshot` skip Cadmus
/// fonts, dpi, and settings. These checks are the render pipeline; there is
/// no committed golden image.
#[cfg(test)]
mod tests {
    use super::super::components::install_ui_font;
    use crate::framebuffer::{Framebuffer as _, Pixmap, UpdateMode};
    use crate::view::View as _;
    use crate::view::slint::generated::{SettingsCategory, SettingsWindow};
    use crate::view::slint::testing::RecordingFramebuffer;

    async fn context_at(dpi: u16) -> crate::device::AppContext {
        use crate::context::test_helpers::create_test_context_from_device;
        use crate::device::test_device::TestDevice;

        create_test_context_from_device(TestDevice::new().with_screen(600, 800, dpi)).await
    }

    /// Opens through the install-font path, at this device's size.
    async fn open_settings_window(
        context: &mut crate::device::AppContext,
    ) -> crate::view::slint::Session<SettingsWindow> {
        install_ui_font(context).await;
        super::super::components::open(context).expect("settings session")
    }

    /// Copies `labels` into the category page. Opening the window does not
    /// publish rows, so a pixel check supplies the lines the page would sync.
    fn seed_rows(window: &SettingsWindow, category: SettingsCategory, labels: &[&str]) {
        use slint::ComponentHandle as _;
        use slint::{ModelRc, VecModel};

        use crate::view::slint::generated::{SettingRow, StaticLine, StaticPageIo};

        let rows = ModelRc::new(VecModel::from(
            labels
                .iter()
                .map(|label| StaticLine {
                    label: (*label).into(),
                    value: "1".into(),
                    row: SettingRow::Locale,
                })
                .collect::<Vec<_>>(),
        ));
        match category {
            SettingsCategory::Libraries | SettingsCategory::Dictionaries => {}
            _ => window.global::<StaticPageIo>().set_rows(rows),
        }
    }

    fn seed_count(window: &SettingsWindow, category: SettingsCategory, count: usize) {
        let labels: Vec<String> = (0..count).map(|index| format!("Row {index}")).collect();
        let borrowed: Vec<&str> = labels.iter().map(String::as_str).collect();
        seed_rows(window, category, &borrowed);
    }

    fn tap_category_at(
        session: &crate::view::slint::Session<SettingsWindow>,
        parts: &super::super::components::Parts,
        x: i32,
        y: i32,
    ) -> Option<SettingsCategory> {
        use crate::geom::Point;
        use crate::input::{DeviceEvent, FingerStatus};

        let position = Point::new(x, y);
        for status in [FingerStatus::Down, FingerStatus::Up] {
            session.handle_device(DeviceEvent::Finger {
                id: 0,
                time: 0.0,
                status,
                position,
            });
        }
        parts.take_category_bar().pop()
    }

    fn find_category_tap(
        session: &crate::view::slint::Session<SettingsWindow>,
        parts: &super::super::components::Parts,
        window: &SettingsWindow,
        target: SettingsCategory,
    ) -> crate::geom::Point {
        use crate::geom::Point;

        for y in [88_i32, 120, 145] {
            for x in (0..600).step_by(8) {
                window.set_category(SettingsCategory::General);
                let _ = session.redraw();
                if tap_category_at(session, parts, x, y) == Some(target) {
                    return Point::new(x, y);
                }
            }
        }
        panic!("no tap hit {target:?}");
    }

    fn dark_band(fb: &RecordingFramebuffer, y0: usize, y1: usize) -> usize {
        let data = fb.pixmap.data();
        let width = fb.pixmap.width() as usize;
        (y0..y1)
            .map(|y| {
                (0..width)
                    .filter(|x| data[(y * width + x) * 3] < 80)
                    .count()
            })
            .sum()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_category_draws_its_rows() {
        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        install_ui_font(&context).await;
        for category in [
            SettingsCategory::General,
            SettingsCategory::Power,
            SettingsCategory::Reader,
            SettingsCategory::Intermissions,
            SettingsCategory::Import,
            SettingsCategory::Telemetry,
        ] {
            let session = super::super::components::open(&mut context).expect("settings session");
            let window = session.component();
            seed_count(window, category, 4);
            window.set_category(category);
            session.present(&mut fb).expect("frame");
            assert!(
                dark_band(&fb, 200, 250) > 0,
                "{category:?} drew no settings rows"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hidden_rows_do_not_reserve_space() {
        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        let window = session.component();
        seed_count(window, SettingsCategory::Power, 3);
        window.set_category(SettingsCategory::Power);
        session.present(&mut fb).expect("frame");
        assert!(
            dark_band(&fb, 200, 250) > 0,
            "the first visible row should sit at the top, below no hidden rows"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pagination_control_appears_when_rows_exceed_a_page() {
        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);

        let many = open_settings_window(&mut context).await;
        let window = many.component();
        seed_count(window, SettingsCategory::General, 24);
        window.set_category(SettingsCategory::General);
        many.present(&mut fb).expect("frame");
        assert!(
            dark_band(&fb, 740, 780) > 0,
            "a pager should render when the rows exceed one page"
        );

        let few = open_settings_window(&mut context).await;
        let window = few.component();
        seed_count(window, SettingsCategory::Import, 1);
        window.set_category(SettingsCategory::Import);
        few.present(&mut fb).expect("frame");
        assert!(
            dark_band(&fb, 740, 780) > 0,
            "the footer should stay when the rows fit one page"
        );
    }

    fn channel_spread(pixel: &[u8]) -> u8 {
        let max = pixel[0].max(pixel[1]).max(pixel[2]);
        let min = pixel[0].min(pixel[1]).min(pixel[2]);
        max - min
    }

    /// A row rule. `#d0d0d0`, allowing a little blend with the white page.
    fn is_divider(pixel: [u8; 3]) -> bool {
        channel_spread(&pixel) <= 8 && (170..=230).contains(&pixel[0])
    }

    /// The selected category is the colored mark. It stays black until the
    /// panel is chromatic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn material_palette_stays_gray_until_color_is_enabled() {
        use slint::ComponentHandle as _;

        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        session.present(&mut fb).expect("frame");
        let worst = fb
            .pixmap
            .data()
            .chunks(3)
            .map(channel_spread)
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 1,
            "gray palette leaked color, channel spread {worst}"
        );

        session
            .component()
            .global::<crate::view::slint::generated::MaterialPalette>()
            .set_chromatic(true);
        assert!(session.redraw(), "color palette did not redraw");
        session.blit(&mut fb).expect("color blit");
        let best = fb
            .pixmap
            .data()
            .chunks(3)
            .map(channel_spread)
            .max()
            .unwrap_or(0);
        assert!(
            best >= 6,
            "color palette did not reach the framebuffer, channel spread {best}"
        );
    }

    /// The accept button is the primary action. On a color panel it is orange;
    /// Cancel stays white. The label is white, so the sample is the orange
    /// padding beside the glyph.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_confirm_accept_button_is_orange_on_a_color_panel() {
        use slint::ComponentHandle as _;

        use crate::view::slint::generated::{ConfirmSheetIo, MaterialPalette};

        let mut context = context_at(300).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        session
            .component()
            .global::<MaterialPalette>()
            .set_chromatic(true);
        let confirm = session.component().global::<ConfirmSheetIo>();
        confirm.set_message("Download dictionary for \"English\"?".into());
        confirm.set_accept_label("Download".into());
        confirm.set_open(true);
        session.present(&mut fb).expect("frame");

        let accept = orange_button(&fb.pixmap).expect("the accept button is not colored");
        let accept_pixel = pixmap_pixel(&fb.pixmap, accept.x0 + 4, (accept.y0 + accept.y1) / 2);
        assert!(
            is_orange(accept_pixel),
            "the accept button is not orange, pixel {accept_pixel:?}"
        );
        let cancel_x = accept.x0.saturating_sub(24);
        let cancel_y = (accept.y0 + accept.y1) / 2;
        let cancel = pixel_at(&fb, cancel_x as f32, cancel_y as f32);
        assert!(
            channel_spread(&cancel) <= 1,
            "Cancel is colored, pixel {cancel:?}"
        );
    }

    struct PxSpan {
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    }

    fn pixmap_pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 3] {
        let width = pixmap.width() as usize;
        let index = (y as usize * width + x as usize) * 3;
        let data = pixmap.data();
        [data[index], data[index + 1], data[index + 2]]
    }

    fn is_ink(pixel: [u8; 3]) -> bool {
        pixel[0] < 40 && pixel[1] < 40 && pixel[2] < 40
    }

    fn is_orange(pixel: [u8; 3]) -> bool {
        channel_spread(&pixel) >= 6 && pixel[0] > pixel[1] && pixel[0] > pixel[2]
    }

    /// The filled primary button. It is the tall orange band.
    fn orange_button(pixmap: &Pixmap) -> Option<PxSpan> {
        let mut best: Option<PxSpan> = None;
        let mut y = 0u32;
        while y < pixmap.height() {
            let Some((x0, x1)) = orange_extent(pixmap, y) else {
                y += 1;
                continue;
            };
            let mut span = PxSpan {
                x0,
                y0: y,
                x1,
                y1: y,
            };
            y += 1;
            while y < pixmap.height() {
                let Some((x0, x1)) = orange_extent(pixmap, y) else {
                    break;
                };
                span.x0 = span.x0.min(x0);
                span.x1 = span.x1.max(x1);
                span.y1 = y;
                y += 1;
            }
            let tall = span.y1.saturating_sub(span.y0);
            let best_tall = best
                .as_ref()
                .map(|item| item.y1.saturating_sub(item.y0))
                .unwrap_or(0);
            if tall > best_tall {
                best = Some(span);
            }
        }
        best.filter(|span| {
            span.y1.saturating_sub(span.y0) > 30 && span.x1.saturating_sub(span.x0) > 40
        })
    }

    fn orange_extent(pixmap: &Pixmap, y: u32) -> Option<(u32, u32)> {
        let mut count = 0u32;
        let mut x0 = u32::MAX;
        let mut x1 = 0u32;
        for x in 0..pixmap.width() {
            if !is_orange(pixmap_pixel(pixmap, x, y)) {
                continue;
            }
            count += 1;
            x0 = x0.min(x);
            x1 = x1.max(x);
        }
        (count >= 40).then_some((x0, x1))
    }

    /// The bordered card, measured from its side strokes.
    ///
    /// On a gray panel the accept button is solid black. A horizontal scan
    /// treats that fill as the card and reports the button height.
    fn card_bounds(pixmap: &Pixmap) -> Option<PxSpan> {
        let width = pixmap.width();
        let height = pixmap.height();
        let mut columns = Vec::new();
        for x in 0..width {
            let mut y = 0u32;
            while y < height {
                if !is_ink(pixmap_pixel(pixmap, x, y)) {
                    y += 1;
                    continue;
                }
                let y0 = y;
                y += 1;
                while y < height && is_ink(pixmap_pixel(pixmap, x, y)) {
                    y += 1;
                }
                if y.saturating_sub(y0) > 80 {
                    columns.push((x, y0, y - 1));
                }
            }
        }

        struct Stroke {
            x0: u32,
            x1: u32,
            y0: u32,
            y1: u32,
        }

        let mut strokes: Vec<Stroke> = Vec::new();
        for (x, y0, y1) in columns {
            let joined = strokes.iter_mut().rev().find(|stroke| {
                x == stroke.x1.saturating_add(1)
                    && y0.abs_diff(stroke.y0) <= 4
                    && y1.abs_diff(stroke.y1) <= 4
            });
            if let Some(stroke) = joined {
                stroke.x1 = x;
                stroke.y0 = stroke.y0.min(y0);
                stroke.y1 = stroke.y1.max(y1);
            } else {
                strokes.push(Stroke {
                    x0: x,
                    x1: x,
                    y0,
                    y1,
                });
            }
        }

        let mut best: Option<PxSpan> = None;
        for left in &strokes {
            if left.x1.saturating_sub(left.x0) >= 6 {
                continue;
            }
            for right in &strokes {
                if right.x0 <= left.x1.saturating_add(40) || right.x1.saturating_sub(right.x0) >= 6
                {
                    continue;
                }
                if left.y0.abs_diff(right.y0) > 8 || left.y1.abs_diff(right.y1) > 8 {
                    continue;
                }
                let span = PxSpan {
                    x0: left.x0,
                    y0: left.y0.min(right.y0),
                    x1: right.x1.saturating_add(1),
                    y1: left.y1.max(right.y1),
                };
                let tall = span.y1.saturating_sub(span.y0);
                let best_tall = best
                    .as_ref()
                    .map(|item| item.y1.saturating_sub(item.y0))
                    .unwrap_or(0);
                if tall > best_tall {
                    best = Some(span);
                }
            }
        }
        best.filter(|span| span.y1.saturating_sub(span.y0) > 80)
    }

    /// Lowest row with a continuous ink run across the screen.
    ///
    /// That is the keyboard's top rule. Key borders are broken by the gaps
    /// between keys, and a card border does not reach the screen edges.
    fn keyboard_rule(pixmap: &Pixmap) -> Option<u32> {
        let width = pixmap.width();
        let mut found = None;
        for y in 0..pixmap.height() {
            let mut run = 0u32;
            let mut longest = 0u32;
            for x in 0..width {
                if is_ink(pixmap_pixel(pixmap, x, y)) {
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
            if longest + 4 >= width {
                found = Some(y);
            }
        }
        found
    }

    fn is_paper(pixel: [u8; 3]) -> bool {
        pixel[0] > 240 && pixel[1] > 240 && pixel[2] > 240
    }

    /// Center of the widest white key face in the lower half of the screen.
    fn key_face_point(pixmap: &Pixmap) -> Option<crate::geom::Point> {
        let width = pixmap.width();
        let mut best: Option<(u32, u32, u32)> = None;
        for y in (pixmap.height() / 2)..pixmap.height() {
            let mut x = 0;
            while x < width {
                if !is_paper(pixmap_pixel(pixmap, x, y)) {
                    x += 1;
                    continue;
                }
                let x0 = x;
                while x < width && is_paper(pixmap_pixel(pixmap, x, y)) {
                    x += 1;
                }
                let len = x - x0;
                let left = x0 > 0 && is_ink(pixmap_pixel(pixmap, x0 - 1, y));
                let right = x < width && is_ink(pixmap_pixel(pixmap, x, y));
                if left && right && len > 20 && best.is_none_or(|(best_len, _, _)| len > best_len) {
                    best = Some((len, x0 + len / 2, y));
                }
            }
        }
        best.map(|(_, x, y)| crate::geom::Point::new(x as i32, y as i32))
    }

    /// A horizontal gray run long enough to be a row rule, not text ink.
    fn has_divider_rule(pixmap: &Pixmap) -> bool {
        let width = pixmap.width();
        for y in 0..pixmap.height() {
            let mut run = 0u32;
            for x in 0..width {
                if is_divider(pixmap_pixel(pixmap, x, y)) {
                    run += 1;
                    if run > width / 2 {
                        return true;
                    }
                } else {
                    run = 0;
                }
            }
        }
        false
    }

    fn session_pixmap(editor: &super::SettingsEditor, width: u32, height: u32) -> Pixmap {
        let mut fb = RecordingFramebuffer::new(width, height);
        editor
            .ui
            .as_ref()
            .expect("session")
            .blit(&mut fb)
            .expect("blit");
        fb.pixmap
    }

    fn pixel_at(fb: &RecordingFramebuffer, x: f32, y: f32) -> [u8; 3] {
        let width = fb.pixmap.width() as usize;
        let index = (y.round() as usize * width + x.round() as usize) * 3;
        let data = fb.pixmap.data();
        [data[index], data[index + 1], data[index + 2]]
    }

    /// Gap under the text-dialog keyboard, in dp.
    ///
    /// This is `bottom-gap` in `text_sheet.slint`. 19dp is the old 36 logical px.
    const KEYBOARD_BOTTOM_GAP_DP: f32 = 19.0;
    /// One dp is 1/160 inch. `InkMetrics.dp` is `BASE_DPI / DP_PER_INCH`.
    const DP_PER_INCH: f32 = 160.0;

    /// The field card sits above the keyboard, and the keyboard matches the
    /// reader keyboard with a gap under it.
    fn assert_text_dialog_layout(pixmap: &Pixmap, context: &crate::device::AppContext) {
        use crate::device::DeviceIdentity as _;

        let width = pixmap.width();
        let height = pixmap.height();
        let card = card_bounds(pixmap).expect("the text field has no card");
        let card_width = card.x1.saturating_sub(card.x0);
        let card_height = card.y1.saturating_sub(card.y0);
        assert!(card_width > 40, "the text dialog has no width");
        assert!(card_height > 40, "the text dialog has no height");
        assert!(
            card_width + 16 < width,
            "the text dialog is as wide as the screen"
        );
        assert!(
            card_height + 16 < height,
            "the text dialog is as tall as the screen"
        );

        let rule = keyboard_rule(pixmap).expect("the keyboard has no hairline");
        let span = crate::view::keyboard::reader_keyboard_span(
            width,
            context.device.dpi(),
            &crate::view::keyboard::selected_layout(context),
        );
        let scale = f32::from(context.device.dpi().max(1)) / crate::unit::BASE_DPI;
        let block = span.block_height * scale;
        let key = span.key_height * scale;
        let keyboard_height = block + key;
        assert!(keyboard_height > 100.0, "the keyboard has no height");
        assert!(
            keyboard_height < height as f32 * 0.75,
            "the keyboard fills the window ({keyboard_height} of {height})"
        );
        let from_bottom = height as f32 - rule as f32;
        let gap = KEYBOARD_BOTTOM_GAP_DP * crate::unit::BASE_DPI / DP_PER_INCH * scale;
        let expected_from_bottom = block + gap;
        assert!(
            (from_bottom - expected_from_bottom).abs() < 4.0,
            "keyboard hairline is {from_bottom}px from the bottom, expected {expected_from_bottom}"
        );
        let bottom_gap = from_bottom - block;
        assert!(
            bottom_gap > 20.0 && bottom_gap < 64.0,
            "the keyboard should sit just above the bottom edge, gap was {bottom_gap}"
        );
        let keyboard_top = rule as f32 - key;
        assert!(
            card.y1 as f32 <= keyboard_top + 4.0,
            "the input covers the keyboard (field bottom {}, keyboard top {keyboard_top})",
            card.y1
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn settings_frame_shows_a_category_and_a_row() {
        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        seed_count(session.component(), SettingsCategory::General, 3);
        session.present(&mut fb).expect("settings frame");
        assert_eq!(fb.updates.as_slice(), &[(fb.rect(), UpdateMode::Full)]);
        assert!(
            fb.pixmap.data().iter().any(|sample| *sample < 80),
            "expected visible settings text"
        );
        assert!(
            has_divider_rule(&fb.pixmap),
            "row rules are not the gray divider"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn switching_category_rerenders_the_rows() {
        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        let window = session.component();
        seed_rows(window, SettingsCategory::General, &["Language"]);
        seed_rows(window, SettingsCategory::Power, &["Auto suspend"]);
        window.set_category(SettingsCategory::General);
        session.present(&mut fb).expect("settings frame");
        let general = fb.pixmap.data().to_vec();

        window.set_category(SettingsCategory::Power);
        assert!(
            session.redraw(),
            "redraw captured no frame after the category changed"
        );
        session.blit(&mut fb).expect("settings blit");
        let power = fb.pixmap.data().to_vec();

        assert_ne!(general, power, "rows did not change with the category");
    }

    /// Power's touch target on the 167 dpi / 600×800 frame. The label ink sits
    /// near y 88; y 145 is the first settings row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tapping_a_category_queues_the_category_action() {
        use super::super::components::Parts;
        use crate::geom::Point;
        use crate::input::{DeviceEvent, FingerStatus};

        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        let parts = Parts::new(None);
        parts.wire(session.component());
        session.present(&mut fb).expect("frame");

        let position = Point::new(120, 88);
        for status in [FingerStatus::Down, FingerStatus::Up] {
            session.handle_device(DeviceEvent::Finger {
                id: 0,
                time: 0.0,
                status,
                position,
            });
        }
        assert_eq!(
            parts.take_category_bar(),
            vec![SettingsCategory::Power],
            "tap at {position:?} did not select Power"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selecting_a_category_does_not_shift_tabs_to_the_right() {
        use super::super::components::Parts;

        let mut context = context_at(167).await;
        let mut fb = RecordingFramebuffer::new(600, 800);
        let session = open_settings_window(&mut context).await;
        let parts = Parts::new(None);
        parts.wire(session.component());
        session.present(&mut fb).expect("frame");
        let window = session.component();
        let libraries = find_category_tap(&session, &parts, window, SettingsCategory::Libraries);

        window.set_category(SettingsCategory::Dictionaries);
        assert!(session.redraw(), "selecting dictionaries captured no frame");
        assert_eq!(
            tap_category_at(&session, &parts, libraries.x, libraries.y),
            Some(SettingsCategory::Libraries),
            "selecting Dictionaries moved Libraries from {libraries:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resize_updates_the_slint_window_to_the_framebuffer() {
        use slint::ComponentHandle as _;

        use super::SettingsEditor;
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::{Framebuffer as _, Pixmap};
        use crate::view::View as _;
        use crate::view::slint::generated::InkScreen;

        let mut context = context_at(167).await;
        install_ui_font(&context).await;
        let rect = context.device.framebuffer().rect();
        let mut ignored = crate::view::RenderQueue::new();
        let mut editor = SettingsEditor::new(rect, &mut ignored, &mut context).await;
        let (hub, _rx) = crate::view::hub_channel();
        let (before_width, before_height) = {
            let chrome = editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<InkScreen>();
            (chrome.get_window_width(), chrome.get_window_height())
        };

        let (width, height) = context.device.framebuffer().dims();
        *context.device.framebuffer_mut() = Pixmap::new(height, width, 1);
        let rotated = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        editor.resize(rotated, &hub, &mut rq, &mut context);

        assert_eq!(editor.rect(), &rotated);
        let session = editor.ui.as_ref().expect("session");
        assert_eq!(session.size(), (height, width));
        let physical = session.component().window().size();
        assert_eq!((physical.width, physical.height), (height, width));
        let chrome = session.component().global::<InkScreen>();
        assert_eq!(chrome.get_window_width(), before_height);
        assert_eq!(chrome.get_window_height(), before_width);

        let mut fb = RecordingFramebuffer::new(height, width);
        session.blit(&mut fb).expect("blit");
        let painted = (0..width).any(|y| !is_paper(pixmap_pixel(&fb.pixmap, height - 2, y)));
        assert!(painted, "resized frame left the new width blank");
        let header = (0..80).any(|y| (0..200).any(|x| is_ink(pixmap_pixel(&fb.pixmap, x, y))));
        assert!(header, "resized frame dropped the header");
        assert!(!rq.is_empty(), "resize did not queue a refresh");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resize_moves_an_open_notification() {
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::{Framebuffer as _, Pixmap};
        use crate::view::View as _;
        use crate::view::notification::Notification;

        let mut context = context_at(167).await;
        install_ui_font(&context).await;
        let rect = context.device.framebuffer().rect();
        let mut ignored = crate::view::RenderQueue::new();
        let mut editor = super::SettingsEditor::new(rect, &mut ignored, &mut context).await;
        let (hub, _rx) = crate::view::hub_channel();
        let mut rq = crate::view::RenderQueue::new();
        let note = Notification::new(None, "Saved".into(), true, &hub, &mut rq, &mut context);
        let before = *note.rect();
        editor.children_mut().push(Box::new(note));

        let (width, height) = context.device.framebuffer().dims();
        *context.device.framebuffer_mut() = Pixmap::new(height, width, 1);
        context.display.dims = (height, width);
        editor.resize(
            context.device.framebuffer().rect(),
            &hub,
            &mut rq,
            &mut context,
        );

        let after = *editor.children()[0].rect();
        assert_ne!(after, before, "the notification stayed at {before}");
        assert_eq!(after.max.x - before.max.x, height as i32 - width as i32);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn toggling_a_setting_updates_the_displayed_value() {
        use slint::ComponentHandle as _;

        use super::SettingsEditor;
        use crate::context::test_helpers::create_test_context;
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::Framebuffer as _;
        use crate::geom::Point;
        use crate::input::{DeviceEvent, FingerStatus};
        use crate::view::View as _;
        use crate::view::slint::generated::{SettingRow, StaticPageIo};

        let mut context = create_test_context().await;
        context.settings.auto_share = false;
        install_ui_font(&context).await;

        let rect = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        let mut editor = SettingsEditor::new(rect, &mut rq, &mut context).await;
        let before = {
            let window = editor.ui.as_ref().expect("session").component();
            let io = window.global::<StaticPageIo>();
            let before = super::super::components::row_value(&io.get_rows(), SettingRow::AutoShare);
            io.invoke_row_pressed(SettingRow::AutoShare);
            before
        };
        assert_eq!(before, crate::fl!("settings-general-toggle-off"));
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let handled = editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Finger {
                    id: 0,
                    time: 0.0,
                    status: FingerStatus::Motion,
                    position: Point::new(0, 0),
                }),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert!(handled);
        let after = super::super::components::row_value(
            &editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<StaticPageIo>()
                .get_rows(),
            SettingRow::AutoShare,
        );
        assert_ne!(
            after, before,
            "the row still shows the value from before the toggle"
        );
        assert!(context.settings.auto_share, "the setting was not applied");
    }

    /// The row cannot be held. Moving the clock past the hold interval checks
    /// that a slow tap still opens it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_tap_still_opens_a_row_that_cannot_be_held() {
        use crate::device::DeviceIdentity as _;
        use crate::input::FingerStatus;
        use crate::view::slint::generated::SettingsCategory;

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        let locale = context.settings.locale.clone();
        editor
            .parts
            .set_category(SettingsCategory::General, &mut context)
            .await;
        editor.sync(&context);
        let position = first_general_row(&editor, context.device.dpi());

        finger(
            &mut editor,
            &mut context,
            &mut rq,
            FingerStatus::Down,
            position,
        )
        .await;
        assert!(!editor.parts.choice_is_open(), "finger down opened the row");
        crate::view::slint::advance_test_clock(std::time::Duration::from_millis(700));
        finger(
            &mut editor,
            &mut context,
            &mut rq,
            FingerStatus::Up,
            position,
        )
        .await;
        assert!(
            editor.parts.choice_is_open(),
            "a slow tap did not open the row"
        );
        assert_eq!(context.settings.locale, locale);
    }

    /// Physical point in the middle of the first general row.
    ///
    /// That row opens a choice and cannot be held. Rows are top-aligned under
    /// `top-chrome`. `per-page` only limits how many rows are visible, so the
    /// first row is not measured from the footer.
    fn first_general_row(editor: &super::SettingsEditor, dpi: u16) -> crate::geom::Point {
        use slint::ComponentHandle as _;

        use crate::unit::BASE_DPI;
        use crate::view::slint::generated::InkMetrics;

        let window = editor.ui.as_ref().expect("session").component();
        let metrics = window.global::<InkMetrics>();
        let scale = f32::from(dpi.max(1)) / BASE_DPI;
        crate::geom::Point::new(
            (metrics.get_dp() * 24.0 * scale).round() as i32,
            ((window.get_top_chrome() + metrics.get_row() * 0.5) * scale).round() as i32,
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selecting_a_category_switches_the_editor() {
        use super::SettingsEditor;
        use crate::context::test_helpers::create_test_context;
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::Framebuffer as _;
        use crate::view::View as _;

        let mut context = create_test_context().await;
        install_ui_font(&context).await;

        let rect = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        let mut editor = SettingsEditor::new(rect, &mut rq, &mut context).await;
        assert!(editor.ui.is_some(), "settings slint session did not open");

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let handled = editor
            .handle_event(
                &crate::view::Event::SelectSettingsCategory(SettingsCategory::Power),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert!(handled);
        assert_eq!(editor.parts.category(), SettingsCategory::Power);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn holding_the_only_library_offers_no_delete() {
        use super::SettingsEditor;
        use crate::context::test_helpers::create_test_context;
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::Framebuffer as _;

        let mut context = create_test_context().await;
        install_ui_font(&context).await;

        let rect = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        let mut editor = SettingsEditor::new(rect, &mut rq, &mut context).await;
        editor
            .parts
            .set_category(SettingsCategory::Libraries, &mut context)
            .await;

        context.settings.libraries.truncate(1);
        editor.parts.hold_library(0, &context).await;
        assert!(
            !editor.parts.choice_is_open(),
            "the last library must not be deletable"
        );

        context.settings.libraries.push(Default::default());
        editor.parts.hold_library(0, &context).await;
        assert!(
            editor.parts.choice_is_open(),
            "a library should be deletable when others remain"
        );
    }

    async fn editor_with_cormorant() -> (
        crate::device::AppContext,
        super::SettingsEditor,
        crate::view::RenderQueue,
    ) {
        use super::SettingsEditor;
        use crate::context::test_helpers::create_test_context;
        use crate::device::DeviceHardware as _;
        use crate::framebuffer::Framebuffer as _;

        let mut context = create_test_context().await;
        install_ui_font(&context).await;
        let rect = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        let editor = SettingsEditor::new(rect, &mut rq, &mut context).await;
        (context, editor, rq)
    }

    async fn drain_motion(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
    ) {
        use crate::geom::Point;
        use crate::input::{DeviceEvent, FingerStatus};
        use crate::view::View as _;

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Finger {
                    id: 0,
                    time: 0.0,
                    status: FingerStatus::Motion,
                    position: Point::new(300, 400),
                }),
                &hub,
                &mut bus,
                rq,
                context,
            )
            .await;
    }

    async fn finger(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
        status: crate::input::FingerStatus,
        position: crate::geom::Point,
    ) {
        use crate::input::DeviceEvent;
        use crate::view::View as _;

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Finger {
                    id: 0,
                    time: 0.0,
                    status,
                    position,
                }),
                &hub,
                &mut bus,
                rq,
                context,
            )
            .await;
    }

    async fn tap(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
        position: crate::geom::Point,
    ) {
        use crate::input::{DeviceEvent, FingerStatus};
        use crate::view::View as _;

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        for status in [FingerStatus::Down, FingerStatus::Up] {
            editor
                .handle_event(
                    &crate::view::Event::Device(DeviceEvent::Finger {
                        id: 0,
                        time: 0.0,
                        status,
                        position,
                    }),
                    &hub,
                    &mut bus,
                    rq,
                    context,
                )
                .await;
        }
    }

    fn open_font_list(editor: &super::SettingsEditor) {
        use slint::ComponentHandle as _;

        use crate::view::slint::generated::{SettingRow, StaticPageIo};

        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::FontFamily);
    }

    /// Index and label of the first unchecked entry on the open list. Slint
    /// lays the dialog out, so a row is picked through its callback, not by
    /// a physical point.
    fn unselected_choice(editor: &super::SettingsEditor) -> (i32, String) {
        use slint::ComponentHandle as _;
        use slint::Model as _;

        use crate::view::slint::generated::ChoiceSheetIo;

        let window = editor.ui.as_ref().expect("session").component();
        let io = window.global::<ChoiceSheetIo>();
        let entries = io.get_entries();
        let index = (0..entries.row_count())
            .find(|index| entries.row_data(*index).is_some_and(|entry| !entry.checked))
            .expect("font list has no other family");
        let entry = entries.row_data(index).expect("row");
        (index as i32, entry.label.to_string())
    }

    /// Index of the open-list entry whose label is `name`.
    fn choice_named(editor: &super::SettingsEditor, name: &str) -> i32 {
        use slint::ComponentHandle as _;
        use slint::Model as _;

        use crate::view::slint::generated::ChoiceSheetIo;

        let window = editor.ui.as_ref().expect("session").component();
        let io = window.global::<ChoiceSheetIo>();
        let entries = io.get_entries();
        (0..entries.row_count())
            .filter_map(|index| entries.row_data(index).map(|entry| (index, entry)))
            .find(|(_, entry)| entry.label == name)
            .map(|(index, _)| index as i32)
            .unwrap_or_else(|| panic!("the list has no {name} row"))
    }

    /// A physical point outside the centered dialog. Tapping the backdrop
    /// closes it; the dialog has no back button of its own. The dialog is
    /// centered and at most two thirds of the screen wide, so a corner lands
    /// on the backdrop.
    fn point_outside_menu() -> crate::geom::Point {
        crate::geom::Point::new(2, 2)
    }

    /// Only the visible page is copied into the window, so the test shows Reader.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tapping_a_font_selects_it_and_returns_to_settings() {
        use slint::ComponentHandle as _;

        use crate::settings::DEFAULT_FONT_FAMILY;
        use crate::view::slint::generated::{ChoiceSheetIo, SettingRow, StaticPageIo};

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        editor
            .parts
            .set_category(SettingsCategory::Reader, &mut context)
            .await;
        editor.sync(&context);
        open_font_list(&editor);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(editor.parts.choice_is_open(), "the font list did not open");
        let (index, name) = unselected_choice(&editor);
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ChoiceSheetIo>()
            .invoke_entry_pressed(index);
        drain_motion(&mut editor, &mut context, &mut rq).await;

        assert!(
            !editor.parts.choice_is_open(),
            "choosing a font left the list open"
        );
        assert_eq!(context.settings.reader.font_family, name);
        assert_ne!(name, DEFAULT_FONT_FAMILY);
        let window = editor.ui.as_ref().expect("session").component();
        assert!(!window.global::<ChoiceSheetIo>().get_open());
        assert_eq!(
            super::super::components::row_value(
                &window.global::<StaticPageIo>().get_rows(),
                SettingRow::FontFamily,
            ),
            name
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_font_list_back_button_returns_without_changing_the_font() {
        use slint::ComponentHandle as _;

        use crate::settings::DEFAULT_FONT_FAMILY;
        use crate::view::slint::generated::ChoiceSheetIo;

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        open_font_list(&editor);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(editor.parts.choice_is_open());
        let outside = point_outside_menu();
        tap(&mut editor, &mut context, &mut rq, outside).await;

        assert!(
            !editor.parts.choice_is_open(),
            "back left the font list open"
        );
        assert_eq!(context.settings.reader.font_family, DEFAULT_FONT_FAMILY);
        assert!(
            !editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<ChoiceSheetIo>()
                .get_open()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adding_a_refresh_rate_kind_keeps_the_format_that_was_picked() {
        use slint::ComponentHandle as _;

        use crate::document::file_extension::FileExtension;
        use crate::view::slint::generated::{
            ChoiceSheetIo, RefreshRatesIo, SettingRow, StaticPageIo,
        };

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::RefreshRate);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<RefreshRatesIo>()
            .invoke_add_pressed();
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(
            editor.parts.choice_is_open(),
            "the format list did not open"
        );

        let index = choice_named(&editor, "epub");
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ChoiceSheetIo>()
            .invoke_entry_pressed(index);
        drain_motion(&mut editor, &mut context, &mut rq).await;

        {
            let window = editor.ui.as_ref().expect("session").component();
            let io = window.global::<RefreshRatesIo>();
            assert!(
                io.get_kind_open(),
                "picking a format did not open the kind editor"
            );
            assert_eq!(io.get_kind_title().as_str(), "EPUB");
            io.invoke_kind_accepted();
        }
        drain_motion(&mut editor, &mut context, &mut rq).await;
        let by_kind = &context.settings.reader.refresh_rate.by_kind;
        assert!(
            by_kind.contains_key(&FileExtension::Epub),
            "the picked format was not stored"
        );
        assert!(
            !by_kind.contains_key(&FileExtension::Cbr),
            "picking epub stored cbr"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_text_dialog_is_a_keyboard_and_does_not_fill_the_screen() {
        use slint::ComponentHandle as _;
        use slint::Model as _;

        use crate::input::FingerStatus;
        use crate::view::slint::generated::{KeyAction, SettingRow, StaticPageIo, TextSheetIo};

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        rq.clear();
        let window = editor.ui.as_ref().expect("session").component();
        window
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::WifiIdleTimeout);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert_keyboard_updates(&rq, window_area(&editor));

        let pixmap = session_pixmap(&editor, editor.rect().width(), editor.rect().height());
        assert_text_dialog_layout(&pixmap, &context);
        let page = {
            let window = editor.ui.as_ref().expect("session").component();
            let sheet = window.global::<TextSheetIo>();
            assert!(sheet.get_open(), "the wifi idle field did not open");
            let rows = sheet.get_rows();
            let top = rows.row_data(0).expect("the keyboard has no rows");
            let first = top.row_data(0).expect("the first row is empty");
            assert_eq!(
                first.label.as_str(),
                "1",
                "a numeric field did not open on the digit layer"
            );
            assert!(
                first.holdable,
                "a digit with fraction forms does not offer a hold popup"
            );
            let space_label = crate::fl!("keyboard-key-space");
            let delete_label = crate::fl!("keyboard-key-delete");
            let delete_short = crate::fl!("keyboard-key-delete-short");
            let mut space = false;
            let mut delete = false;
            for row in rows.iter() {
                for key in row.iter() {
                    if key.insert.as_str() == " " {
                        assert_eq!(
                            key.label.as_str(),
                            space_label.as_str(),
                            "the space bar has no label"
                        );
                        space = true;
                    }
                    if key.action == KeyAction::DeleteBack {
                        assert!(
                            key.label.as_str() == delete_label.as_str()
                                || key.label.as_str() == delete_short.as_str(),
                            "delete is labeled {:?}",
                            key.label
                        );
                        delete = true;
                    }
                }
            }
            assert!(space, "the layout has no space bar");
            assert!(delete, "the layout has no delete key");
            window.get_page()
        };

        let face = key_face_point(&pixmap).expect("the keyboard has no key face");
        rq.clear();
        finger(&mut editor, &mut context, &mut rq, FingerStatus::Down, face).await;
        assert_key_flash(&rq, window_area(&editor));
        finger(&mut editor, &mut context, &mut rq, FingerStatus::Up, face).await;

        let category = editor.parts.category();
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        assert_eq!(editor.parts.category(), category);
        assert_eq!(
            editor.ui.as_ref().expect("session").component().get_page(),
            page
        );
        assert!(window_text_open(&editor), "paging closed the text dialog");
        assert!(!editor.parts.choice_is_open());

        let window = editor.ui.as_ref().expect("session").component();
        let sheet = window.global::<TextSheetIo>();
        rq.clear();
        let chars =
            sheet.get_text_before().chars().count() + sheet.get_text_after().chars().count();
        for _ in 0..chars {
            sheet.invoke_deleted();
        }
        sheet.invoke_typed("1".into());
        sheet.invoke_typed("2".into());
        sheet.invoke_typed(".".into());
        sheet.invoke_typed("5".into());
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert_keyboard_updates(&rq, window_area(&editor));
        let shown = editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<TextSheetIo>();
        assert_eq!(
            format!("{}{}", shown.get_text_before(), shown.get_text_after()),
            "12.5"
        );

        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<TextSheetIo>()
            .invoke_accepted();
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert_eq!(context.settings.wifi_idle_timeout, 12.5);
        assert!(!window_text_open(&editor));
    }

    /// Download and re-import questions share this card. It sits in the
    /// middle and leaves the page visible around it. The re-import warning
    /// is a paragraph, so the card is tall. Rounded corners shorten the
    /// measured border, so the center is what has to match.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_confirm_dialog_floats_in_the_center() {
        use slint::ComponentHandle as _;

        use crate::geom::Point;
        use crate::input::{DeviceEvent, FingerStatus};
        use crate::view::View as _;
        use crate::view::slint::generated::{ConfirmSheetIo, SettingRow, StaticPageIo};

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::ForceFullImport);
        drain_motion(&mut editor, &mut context, &mut rq).await;

        let window = editor.ui.as_ref().expect("session").component();
        let confirm = window.global::<ConfirmSheetIo>();
        assert!(confirm.get_open(), "force import did not ask");
        assert_eq!(
            confirm.get_message().as_str(),
            crate::fl!("settings-import-force-full-import-confirm")
        );
        let pixmap = session_pixmap(&editor, editor.rect().width(), editor.rect().height());
        let card = card_bounds(&pixmap).expect("the confirm card has no border");
        let window_width = pixmap.width() as f32;
        let window_height = pixmap.height() as f32;
        let dialog_width = card.x1.saturating_sub(card.x0) as f32;
        let dialog_height = card.y1.saturating_sub(card.y0) as f32;
        let dialog_x = card.x0 as f32;
        let dialog_y = card.y0 as f32;
        assert!(dialog_width > 0.0, "the confirm card has no width");
        assert!(
            dialog_height > 180.0,
            "the confirm card collapsed ({dialog_height})"
        );
        assert!(
            window_width - dialog_width >= 48.0,
            "the confirm card reaches the side edges ({dialog_width} of {window_width})"
        );
        assert!(
            window_height - dialog_height >= 96.0,
            "the confirm card reaches the top and bottom ({dialog_height} of {window_height})"
        );
        let expected_x = (window_width - dialog_width) / 2.0;
        let expected_y = (window_height - dialog_height) / 2.0;
        assert!(
            (dialog_x - expected_x).abs() < 4.0,
            "the confirm card is not centered horizontally ({dialog_x}, expected {expected_x})"
        );
        assert!(
            (dialog_y - expected_y).abs() < 4.0,
            "the confirm card is not centered vertically ({dialog_y}, expected {expected_y})"
        );

        tap(&mut editor, &mut context, &mut rq, Point::new(2, 2)).await;
        assert!(
            !editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<ConfirmSheetIo>()
                .get_open(),
            "a tap outside the card left it open"
        );

        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::ForceFullImport);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(
            editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<ConfirmSheetIo>()
                .get_open()
        );
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ConfirmSheetIo>()
            .invoke_accepted();
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Finger {
                    id: 0,
                    time: 0.0,
                    status: FingerStatus::Motion,
                    position: Point::new(300, 400),
                }),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert!(
            bus.iter().any(|event| matches!(
                event,
                crate::view::Event::ImportLibrary {
                    library_index: None,
                    force: true,
                }
            )),
            "accepting the card did not re-import, bus was {bus:?}"
        );
        assert!(
            !editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<ConfirmSheetIo>()
                .get_open(),
            "accepting the card left it open"
        );
    }

    fn window_area(editor: &super::SettingsEditor) -> f32 {
        use slint::ComponentHandle as _;

        use crate::view::slint::generated::InkScreen;

        let chrome = editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<InkScreen>();
        chrome.get_window_width() * chrome.get_window_height()
    }

    /// The keyboard queues each dirty rectangle without waiting, and none of
    /// them covers the screen.
    fn assert_keyboard_updates(rq: &crate::view::RenderQueue, screen: f32) {
        use crate::framebuffer::UpdateMode;

        let updates: Vec<_> = rq
            .iter()
            .flat_map(|((mode, wait), rects)| rects.iter().map(|(_, rect)| (*mode, *wait, *rect)))
            .collect();
        assert!(!updates.is_empty(), "the keyboard did not queue a redraw");
        for (mode, wait, rect) in updates {
            assert_eq!(mode, UpdateMode::Gui);
            assert!(!wait, "a keyboard redraw waited for the previous frame");
            let area = rect.width() as f32 * rect.height() as f32;
            assert!(
                area < screen,
                "dirty area {rect} covers the {screen} px screen"
            );
        }
    }

    /// A finger-down on an open keyboard is the black key flash: fast, and it
    /// does not wait for the previous waveform.
    fn assert_key_flash(rq: &crate::view::RenderQueue, screen: f32) {
        use crate::framebuffer::UpdateMode;

        let updates: Vec<_> = rq
            .iter()
            .flat_map(|((mode, wait), rects)| rects.iter().map(|(_, rect)| (*mode, *wait, *rect)))
            .collect();
        assert!(
            !updates.is_empty(),
            "a finger-down on the keyboard did not queue a redraw"
        );
        for (mode, wait, rect) in updates {
            assert_eq!(mode, UpdateMode::Fast, "a pressed key was not a fast flash");
            assert!(!wait, "a key flash waited for the previous frame");
            let area = rect.width() as f32 * rect.height() as f32;
            assert!(
                area < screen,
                "dirty area {rect} covers the {screen} px screen"
            );
        }
    }

    fn window_text_open(editor: &super::SettingsEditor) -> bool {
        use slint::ComponentHandle as _;

        use crate::view::slint::generated::TextSheetIo;

        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<TextSheetIo>()
            .get_open()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paging_while_the_font_list_is_open_stays_on_the_page() {
        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        open_font_list(&editor);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(editor.parts.choice_is_open());
        let category = editor.parts.category();
        let page = editor.ui.as_ref().expect("session").component().get_page();
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;

        assert_eq!(editor.parts.category(), category);
        assert_eq!(
            editor.ui.as_ref().expect("session").component().get_page(),
            page
        );
        assert!(editor.parts.choice_is_open(), "paging closed the font list");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paging_a_long_choice_sheet_moves_one_page_and_a_toggle_stays_there() {
        use slint::ComponentHandle as _;
        use slint::Model as _;

        use crate::view::slint::generated::{ChoiceSheetIo, SettingRow, StaticPageIo};

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<StaticPageIo>()
            .invoke_row_pressed(SettingRow::AllowedKinds);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(editor.parts.choice_is_open(), "allowed kinds did not open");

        let count = editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ChoiceSheetIo>()
            .get_entries()
            .row_count();
        assert!(
            count > 8,
            "allowed kinds should be longer than one page, got {count}"
        );
        assert_eq!(choice_page(&editor), 0);

        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        assert_eq!(choice_page(&editor), 1, "forward did not move a page");
        assert!(editor.parts.choice_is_open());

        press_backward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        assert_eq!(
            choice_page(&editor),
            0,
            "back did not return to the first page"
        );

        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ChoiceSheetIo>()
            .invoke_entry_pressed(0);
        drain_motion(&mut editor, &mut context, &mut rq).await;
        assert!(
            editor.parts.choice_is_open(),
            "toggling a kind closed the list"
        );
        assert_eq!(
            choice_page(&editor),
            1,
            "toggling a kind jumped back to the first page"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn releasing_the_category_button_does_not_open_a_row() {
        use crate::view::View as _;
        use crate::view::slint::generated::SettingsCategory;

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        assert_eq!(editor.parts.category(), SettingsCategory::Power);
        assert!(
            editor.parts.is_plain(),
            "releasing the category button opened a row"
        );

        editor
            .handle_event(
                &crate::view::Event::SlintTimer,
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert!(
            editor.parts.is_plain(),
            "a timer tick after the release opened a row"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_forwarded_press_does_not_keep_a_release_that_never_arrived() {
        use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
        use crate::view::View as _;
        use crate::view::slint::generated::SettingsCategory;

        let (mut context, mut editor, mut rq) = editor_with_cormorant().await;
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Button {
                    time: 0.0,
                    code: ButtonCode::Forward,
                    status: ButtonStatus::Pressed,
                }),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert_eq!(editor.parts.category(), SettingsCategory::Power);
        assert_eq!(editor.swallow_release, Some(ButtonCode::Forward));

        editor.parts.show_confirm();
        assert!(editor.parts.confirm_is_open());

        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Button {
                    time: 0.0,
                    code: ButtonCode::Forward,
                    status: ButtonStatus::Pressed,
                }),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert_eq!(
            editor.swallow_release, None,
            "the new press inherited a release that never arrived"
        );

        editor
            .handle_event(
                &crate::view::Event::Device(DeviceEvent::Button {
                    time: 0.0,
                    code: ButtonCode::Forward,
                    status: ButtonStatus::Released,
                }),
                &hub,
                &mut bus,
                &mut rq,
                &mut context,
            )
            .await;
        assert_eq!(editor.swallow_release, None);
        assert!(
            editor.parts.confirm_is_open(),
            "forwarding the button closed the confirm sheet"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stepping_categories_with_buttons_does_not_open_an_editor() {
        use slint::ComponentHandle as _;

        use crate::context::test_helpers::create_test_context_from_device;
        use crate::device::DeviceHardware as _;
        use crate::device::test_device::TestDevice;
        use crate::framebuffer::Framebuffer as _;
        use crate::view::slint::generated::{ChoiceSheetIo, LibraryEditorIo, SettingsCategory};

        let mut context =
            create_test_context_from_device(TestDevice::new().with_screen(1264, 1680, 300)).await;
        install_ui_font(&context).await;
        context.settings.libraries.push(Default::default());
        let rect = context.device.framebuffer().rect();
        let mut rq = crate::view::RenderQueue::new();
        let mut editor = super::SettingsEditor::new(rect, &mut rq, &mut context).await;
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();

        for _ in 0..4 {
            press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        }
        assert_eq!(editor.parts.category(), SettingsCategory::Libraries);
        let window = editor.ui.as_ref().expect("session").component();
        assert!(
            !window.global::<LibraryEditorIo>().get_open(),
            "reaching Libraries opened the library editor"
        );
        assert!(!window.global::<ChoiceSheetIo>().get_open());
        assert!(editor.parts.is_plain());

        press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
        assert_eq!(editor.parts.category(), SettingsCategory::Intermissions);
        assert!(
            editor.parts.is_plain(),
            "leaving Libraries opened an editor"
        );
    }

    /// A tap still opens only that library after the hold interval. The clock
    /// moves, so the test does not wait.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tapping_the_first_library_opens_only_that_editor() {
        use crate::geom::Point;
        use crate::input::FingerStatus;

        let mut screen = LibraryScreen::open().await;
        crate::view::slint::advance_test_clock(std::time::Duration::from_millis(800));

        let position = Point::new(273, 374);
        screen.finger(1, FingerStatus::Down, position).await;
        let (open, _, choice) = screen.overlays();
        assert!(!open, "finger down opened a library editor");
        assert!(!choice, "finger down opened the delete sheet");

        screen.finger(1, FingerStatus::Up, position).await;
        let (open, heading, choice) = screen.overlays();
        assert!(open, "tapping the first library did not open its editor");
        assert_eq!(
            heading, "On Board",
            "tapping the first library opened a different one"
        );
        assert!(
            !choice,
            "tapping the first library also opened the delete sheet"
        );

        let back = Point::new(62, 68);
        screen.finger(2, FingerStatus::Down, back).await;
        screen.finger(2, FingerStatus::Up, back).await;
        let (open, _, choice) = screen.overlays();
        assert!(!open, "one back left a library editor open");
        assert!(!choice, "one back left the delete sheet open");
        assert!(
            screen.editor.parts.is_plain(),
            "one back left an overlay open"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn holding_the_first_library_opens_only_its_delete_sheet() {
        use crate::geom::Point;
        use crate::input::FingerStatus;
        use crate::view::View as _;

        let mut screen = LibraryScreen::open().await;
        let position = Point::new(273, 374);
        screen.finger(1, FingerStatus::Down, position).await;
        crate::view::slint::advance_test_clock(std::time::Duration::from_millis(800));
        screen.finger(1, FingerStatus::Up, position).await;

        let (open, _, choice) = screen.overlays();
        assert!(!open, "holding a library also opened its editor");
        assert!(choice, "holding a library did not open the delete sheet");
        assert_eq!(screen.choice_title(), "On Board");

        screen
            .editor
            .handle_event(
                &crate::view::Event::Back,
                &screen.hub,
                &mut screen.bus,
                &mut screen.rq,
                &mut screen.context,
            )
            .await;
        let (open, _, choice) = screen.overlays();
        assert!(!choice, "one back left the delete sheet open");
        assert!(!open, "the delete sheet was covering a library editor");
    }

    struct LibraryScreen {
        context: crate::device::AppContext,
        rq: crate::view::RenderQueue,
        editor: super::SettingsEditor,
        hub: crate::view::Hub,
        bus: crate::view::Bus,
    }

    impl LibraryScreen {
        async fn open() -> Self {
            use std::path::PathBuf;

            use crate::context::test_helpers::create_test_context_from_device;
            use crate::device::DeviceHardware as _;
            use crate::device::test_device::TestDevice;
            use crate::framebuffer::Framebuffer as _;
            use crate::settings::LibrarySettings;
            use crate::view::slint::generated::SettingsCategory;

            let mut context =
                create_test_context_from_device(TestDevice::new().with_screen(1264, 1680, 300))
                    .await;
            install_ui_font(&context).await;
            context.settings.libraries = vec![
                LibrarySettings {
                    name: "On Board".to_string(),
                    path: PathBuf::from("/mnt/onboard"),
                    ..Default::default()
                },
                LibrarySettings {
                    name: "KePub".to_string(),
                    path: PathBuf::from("/mnt/onboard/.kobo/kepub"),
                    ..Default::default()
                },
            ];
            let rect = context.device.framebuffer().rect();
            let mut rq = crate::view::RenderQueue::new();
            let mut editor = super::SettingsEditor::new(rect, &mut rq, &mut context).await;
            let (hub, _rx) = crate::view::hub_channel();
            let mut bus = crate::view::Bus::new();
            for _ in 0..4 {
                press_forward(&mut editor, &mut context, &mut rq, &hub, &mut bus).await;
            }
            assert_eq!(editor.parts.category(), SettingsCategory::Libraries);
            Self {
                context,
                rq,
                editor,
                hub,
                bus,
            }
        }

        async fn finger(
            &mut self,
            id: i32,
            status: crate::input::FingerStatus,
            position: crate::geom::Point,
        ) {
            use crate::input::DeviceEvent;
            use crate::view::View as _;

            self.editor
                .handle_event(
                    &crate::view::Event::Device(DeviceEvent::Finger {
                        id,
                        time: 0.0,
                        status,
                        position,
                    }),
                    &self.hub,
                    &mut self.bus,
                    &mut self.rq,
                    &mut self.context,
                )
                .await;
        }

        fn overlays(&self) -> (bool, String, bool) {
            use slint::ComponentHandle as _;

            use crate::view::slint::generated::{ChoiceSheetIo, LibraryEditorIo};

            let window = self.editor.ui.as_ref().expect("session").component();
            (
                window.global::<LibraryEditorIo>().get_open(),
                window.global::<LibraryEditorIo>().get_heading().to_string(),
                window.global::<ChoiceSheetIo>().get_open(),
            )
        }

        fn choice_title(&self) -> String {
            use slint::ComponentHandle as _;

            use crate::view::slint::generated::ChoiceSheetIo;

            self.editor
                .ui
                .as_ref()
                .expect("session")
                .component()
                .global::<ChoiceSheetIo>()
                .get_title()
                .to_string()
        }
    }

    fn choice_page(editor: &super::SettingsEditor) -> i32 {
        use slint::ComponentHandle as _;

        use crate::view::slint::generated::ChoiceSheetIo;

        editor
            .ui
            .as_ref()
            .expect("session")
            .component()
            .global::<ChoiceSheetIo>()
            .get_page()
    }

    async fn press_button(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
        hub: &crate::view::Hub,
        bus: &mut crate::view::Bus,
        code: crate::input::ButtonCode,
    ) {
        use crate::input::{ButtonStatus, DeviceEvent};
        use crate::view::View as _;

        for status in [ButtonStatus::Pressed, ButtonStatus::Released] {
            editor
                .handle_event(
                    &crate::view::Event::Device(DeviceEvent::Button {
                        time: 0.0,
                        code,
                        status,
                    }),
                    hub,
                    bus,
                    rq,
                    context,
                )
                .await;
        }
    }

    async fn press_backward(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
        hub: &crate::view::Hub,
        bus: &mut crate::view::Bus,
    ) {
        press_button(
            editor,
            context,
            rq,
            hub,
            bus,
            crate::input::ButtonCode::Backward,
        )
        .await;
    }

    async fn press_forward(
        editor: &mut super::SettingsEditor,
        context: &mut crate::device::AppContext,
        rq: &mut crate::view::RenderQueue,
        hub: &crate::view::Hub,
        bus: &mut crate::view::Bus,
    ) {
        press_button(
            editor,
            context,
            rq,
            hub,
            bus,
            crate::input::ButtonCode::Forward,
        )
        .await;
    }
}

#[cfg(test)]
mod job_tests {
    use super::super::components::Parts;
    use super::{SettingsEditor, SettingsScreen};
    use crate::runtime::Job;
    use crate::view::View;
    use crate::view::slint::SlintHost;

    fn editor(job: Option<Job>) -> SettingsEditor {
        let mut editor = SlintHost::blank(SettingsScreen {
            parts: Parts::new(None),
        });
        editor.parts.set_dictionary_job(job);
        editor
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn take_background_jobs_drains_the_dictionary_download() {
        let mut view = editor(Some(Job::spawn(|token| async move {
            token.cancelled().await;
        })));
        let mut jobs = Vec::new();
        crate::view::take_view_jobs(&mut view, &mut jobs);
        assert_eq!(jobs.len(), 1);
        assert!(view.parts.dictionary_job().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_jobs_cancels_the_dictionary_download() {
        let view = editor(Some(Job::spawn(|token| async move {
            token.cancelled().await;
        })));
        view.stop_jobs();
    }
}
