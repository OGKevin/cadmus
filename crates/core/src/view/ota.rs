use super::dialog::Dialog;
use super::input_field::InputField;
use super::label::Label;
use super::notification::Notification;
use super::toggleable_keyboard::ToggleableKeyboard;
use super::{
    Align, Bus, Event, Hub, Id, NotificationEvent, RenderData, RenderQueue, UpdateMode, View,
    ViewId, ID_FEEDER,
};
use crate::color::WHITE;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::ota::{OtaClient, OtaProgress};
use crate::unit::scale_by_dpi;
use crate::view::filler::Filler;
use crate::view::BIG_BAR_HEIGHT;
use secrecy::SecretString;
use std::thread;
use tracing::{error, info};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum OtaViewId {
    Main,
    SourceSelection,
    PrInput,
}

/// Attempts to show the OTA update view with validation checks.
///
/// This function validates prerequisites before showing the OTA view:
/// - Checks if WiFi is enabled
/// - Verifies GitHub token is configured in settings
///
/// If validation fails, a notification is added to the view hierarchy instead.
///
/// # Arguments
///
/// * `view` - The parent view to add either OTA view or notification to
/// * `hub` - Event hub for sending events
/// * `rq` - Render queue for UI updates
/// * `context` - Application context containing settings and WiFi state
///
/// # Returns
///
/// `true` if the OTA view was successfully shown, `false` if validation failed
/// and a notification was shown instead.
#[cfg_attr(
    feature = "otel",
    tracing::instrument(
        skip_all, ret(level=tracing::Level::TRACE),
        ret(level = tracing::Level::TRACE)
    )
)]
pub fn show_ota_view(
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut Context,
) -> bool {
    #[cfg(feature = "otel")]
    tracing::trace!("showing ota view");

    // TODO(ogkevin): This only checks if WiFi is enabled in settings, not if there's an actual
    // connection or internet access. Should verify actual network connectivity.
    // See: https://github.com/OGKevin/cadmus/issues/69
    if !context.settings.wifi {
        let notif = Notification::new(
            None,
            "WiFi must be enabled to check for updates.".to_string(),
            false,
            hub,
            rq,
            context,
        );
        view.children_mut().push(Box::new(notif) as Box<dyn View>);
        return false;
    }

    if context.settings.ota.github_token.is_none() {
        let notif = Notification::new(
            None,
            "GitHub token not configured. Add [ota] github-token to Settings.toml".to_string(),
            false,
            hub,
            rq,
            context,
        );
        view.children_mut().push(Box::new(notif) as Box<dyn View>);
        return false;
    }

    let ota_view = OtaView::new(context.settings.ota.github_token.clone().unwrap(), context);
    view.children_mut()
        .push(Box::new(ota_view) as Box<dyn View>);
    true
}

/// UI view for downloading and installing OTA updates from GitHub pull requests.
///
/// Manages two screens:
/// 1. Source selection dialog - asks where to download from (PR, Stable, etc.)
/// 2. PR input screen - prompts for PR number input
///
/// The view transitions between screens based on user selections.
///
/// # Security
///
/// The GitHub token is securely stored using `SecretString` to prevent
/// accidental exposure in logs or debug output.
pub struct OtaView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    view_id: ViewId,
    github_token: SecretString,
    keyboard_index: Option<usize>,
}

impl OtaView {
    /// Creates a new OTA view with the configured GitHub token.
    ///
    /// Initially displays the source selection dialog asking where to
    /// download updates from.
    ///
    /// # Arguments
    ///
    /// * `github_token` - GitHub personal access token wrapped in `SecretString`
    ///   for secure handling
    /// * `context` - Application context containing fonts and device information
    #[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
    pub fn new(github_token: SecretString, context: &mut Context) -> OtaView {
        let id = ID_FEEDER.next();
        let view_id = ViewId::Ota(OtaViewId::Main);
        let (width, height) = CURRENT_DEVICE.dims;

        let mut children: Vec<Box<dyn View>> = Vec::new();

        children.push(Box::new(Filler::new(
            rect![0, 0, width as i32, height as i32],
            WHITE,
        )));

        let source_dialog = Self::build_source_selection_dialog(context);
        children.push(Box::new(source_dialog));

        OtaView {
            id,
            rect: rect![0, 0, width as i32, height as i32],
            children,
            view_id,
            github_token,
            keyboard_index: None,
        }
    }

    /// Builds the source selection dialog.
    #[inline]
    fn build_source_selection_dialog(context: &mut Context) -> Dialog {
        Dialog::builder(
            ViewId::Ota(OtaViewId::SourceSelection),
            "Where to check for updates?".to_string(),
        )
        .add_button("PR Build", Event::Show(ViewId::Ota(OtaViewId::PrInput)))
        .build(context)
    }

    /// Builds the PR input screen with title, input field, and keyboard.
    fn build_pr_input_screen(&mut self, context: &mut Context) {
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = CURRENT_DEVICE.dims;

        self.children.clear();

        self.children.push(Box::new(Filler::new(
            rect![0, 0, width as i32, height as i32],
            WHITE,
        )));

        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let x_height = font.x_heights.0 as i32;
        let padding = font.em() as i32;

        let dialog_width = scale_by_dpi(width as f32, dpi) as i32;
        let dialog_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        let dx = (width as i32 - dialog_width) / 2;
        let dy = (height as i32) / 3 - dialog_height / 2;
        let rect = rect![dx, dy, dx + dialog_width, dy + dialog_height];

        let title_rect = rect![
            rect.min.x + padding,
            rect.min.y + padding,
            rect.max.x - padding,
            rect.min.y + padding + 3 * x_height
        ];
        let title = Label::new(
            title_rect,
            "Download Build from PR".to_string(),
            Align::Center,
        );
        self.children.push(Box::new(title));

        let input_rect = rect![
            rect.min.x + 2 * padding,
            rect.min.y + padding + 4 * x_height,
            rect.max.x - 2 * padding,
            rect.min.y + padding + 8 * x_height
        ];
        let input = InputField::new(input_rect, ViewId::Ota(OtaViewId::PrInput));
        self.children.push(Box::new(input));

        let screen_rect = rect![0, 0, width as i32, height as i32];
        let keyboard = ToggleableKeyboard::new(screen_rect, true);
        self.children.push(Box::new(keyboard));
        self.keyboard_index = Some(self.children.len() - 1);

        self.rect = rect![0, 0, width as i32, height as i32];
    }

    /// Toggles keyboard visibility based on focus state.
    ///
    /// # Arguments
    ///
    /// * `visible` - Whether the keyboard should be visible
    /// * `hub` - Event hub for sending events
    /// * `rq` - Render queue for UI updates
    /// * `context` - Application context
    fn toggle_keyboard(
        &mut self,
        visible: bool,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        if let Some(idx) = self.keyboard_index {
            if let Some(keyboard) = self.children.get_mut(idx) {
                if let Some(kb) = keyboard.downcast_mut::<ToggleableKeyboard>() {
                    kb.set_visible(visible, hub, rq, context);
                }
            }
        }
    }

    /// Handles submission of PR number from input field.
    ///
    /// Validates the input, initiates download if valid, and closes the view.
    ///
    /// # Arguments
    ///
    /// * `text` - The input text to parse as PR number
    /// * `hub` - Event hub for sending notifications
    fn handle_pr_submission(&mut self, text: &str, hub: &Hub) {
        if let Ok(pr_number) = text.trim().parse::<u32>() {
            hub.send(Event::Notification(NotificationEvent::Show(format!(
                "Downloading PR #{} build...",
                pr_number
            ))))
            .ok();
            self.start_download(pr_number, hub);
            hub.send(Event::Close(self.view_id)).ok();
        } else {
            hub.send(Event::Notification(NotificationEvent::Show(
                "Invalid PR number".to_string(),
            )))
            .ok();
        }
    }

    /// Handles tap gesture outside the dialog and keyboard areas.
    ///
    /// Closes the view when user taps outside to dismiss.
    ///
    /// # Arguments
    ///
    /// * `tap_position` - The position where the tap occurred
    /// * `context` - Application context containing keyboard rectangle
    /// * `hub` - Event hub for sending close event
    fn handle_outside_tap(&self, tap_position: crate::geom::Point, context: &Context, hub: &Hub) {
        if !self.rect.includes(tap_position)
            && !context.kb_rect.includes(tap_position)
            && !context.kb_rect.is_empty()
        {
            hub.send(Event::Close(self.view_id)).ok();
        }
    }

    /// Initiates the download process in a background thread.
    ///
    /// Spawns a thread that:
    /// 1. Creates an OTA client
    /// 2. Downloads the artifact for the specified PR
    /// 3. Extracts and deploys KoboRoot.tgz
    /// 4. Sends notification events on success or failure
    ///
    /// # Arguments
    ///
    /// * `pr_number` - The GitHub pull request number to download
    /// * `hub` - Event hub for sending notifications and status updates
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub)))]
    fn start_download(&mut self, pr_number: u32, hub: &Hub) {
        let github_token = self.github_token.clone();
        let hub2 = hub.clone();

        thread::spawn(move || {
            let client = match OtaClient::new(github_token) {
                Ok(c) => c,
                Err(e) => {
                    error!("[OTA] Failed to create github client {:?}", e);
                    let error_msg = format!("Failed to create client: {}", e);
                    hub2.send(Event::Notification(NotificationEvent::Show(error_msg)))
                        .ok();
                    return;
                }
            };

            let notify_id = ViewId::MessageNotif(ID_FEEDER.next());
            hub2.send(Event::Notification(NotificationEvent::ShowPinned(
                notify_id,
                "Starting update download".to_string(),
            )))
            .ok();
            hub2.send(Event::Notification(NotificationEvent::UpdateProgress(
                notify_id, 0,
            )))
            .ok();

            let download_result = client.download_pr_artifact(pr_number, |ota_progress| {
                if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                    let progress = (downloaded as f32 / total as f32) * 100.0;
                    let msg = format!("Downloading update: {}%", progress as u8);
                    hub2.send(Event::Notification(NotificationEvent::UpdateText(
                        notify_id, msg,
                    )))
                    .ok();
                    hub2.send(Event::Notification(NotificationEvent::UpdateProgress(
                        notify_id,
                        progress as u8,
                    )))
                    .ok();
                }
            });

            hub2.send(Event::Close(notify_id)).ok();

            match download_result {
                Ok(zip_path) => {
                    info!("[OTA] Download completed, starting extraction...");

                    match client.extract_and_deploy(zip_path) {
                        Ok(_) => {
                            hub2.send(Event::Notification(NotificationEvent::Show(
                                "Update installed! Reboot to apply.".to_string(),
                            )))
                            .ok();
                        }
                        Err(e) => {
                            error!("[OTA] Deployment error: {:?}", e);
                            let error_msg = format!("Deployment failed: {}", e);
                            hub2.send(Event::Notification(NotificationEvent::Show(error_msg)))
                                .ok();
                        }
                    }
                }
                Err(e) => {
                    error!("[OTA] Download error: {:?}", e);
                    let error_msg = format!("Download failed: {}", e);
                    hub2.send(Event::Notification(NotificationEvent::Show(error_msg)))
                        .ok();
                }
            }
        });
    }
}

impl View for OtaView {
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Show(ViewId::Ota(OtaViewId::PrInput)) => {
                #[cfg(feature = "otel")]
                tracing::trace!("Showing PR input screen");

                self.build_pr_input_screen(context);
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                self.toggle_keyboard(true, hub, rq, context);
                hub.send(Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput))))
                    .ok();
                true
            }
            Event::Focus(None) => {
                self.toggle_keyboard(false, hub, rq, context);
                true
            }
            Event::Focus(Some(ViewId::Ota(_))) => true,
            Event::Submit(ViewId::Ota(OtaViewId::PrInput), ref text) => {
                self.toggle_keyboard(false, hub, rq, context);
                self.handle_pr_submission(text, hub);
                true
            }
            Event::Gesture(GestureEvent::Tap(center)) => {
                self.handle_outside_tap(center, context, hub);
                true
            }
            _ => false,
        }
    }
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, _fb, _fonts, _rect), fields(rect = ?_rect)))]
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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
        Some(self.view_id)
    }

    fn resize(
        &mut self,
        _rect: Rectangle,
        _hub: &Hub,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::view::handle_event;
    use crate::view::keyboard::Keyboard;
    use std::collections::VecDeque;
    use std::sync::mpsc::channel;

    fn create_ota_view(context: &mut Context) -> OtaView {
        OtaView::new(SecretString::from("test-token"), context)
    }

    /// A minimal parent view that mimics Home/Reader keyboard behavior.
    ///
    /// When it receives `Event::Focus(Some(_))`, it inserts a Keyboard
    /// child — exactly like Home and Reader do. This lets us assert that
    /// the OtaView prevents the focus event from reaching the parent.
    struct FakeParentView {
        id: Id,
        rect: Rectangle,
        children: Vec<Box<dyn View>>,
    }

    impl FakeParentView {
        fn new(rect: Rectangle) -> Self {
            FakeParentView {
                id: ID_FEEDER.next(),
                rect,
                children: Vec::new(),
            }
        }

        fn has_keyboard(&self) -> bool {
            self.children
                .iter()
                .any(|c| c.downcast_ref::<Keyboard>().is_some())
        }
    }

    impl View for FakeParentView {
        fn handle_event(
            &mut self,
            evt: &Event,
            _hub: &Hub,
            _bus: &mut Bus,
            _rq: &mut RenderQueue,
            context: &mut Context,
        ) -> bool {
            match *evt {
                Event::Focus(Some(_)) => {
                    let mut kb_rect = rect![
                        self.rect.min.x,
                        self.rect.max.y - 300,
                        self.rect.max.x,
                        self.rect.max.y - 66
                    ];
                    let keyboard = Keyboard::new(&mut kb_rect, false, context);
                    self.children.push(Box::new(keyboard) as Box<dyn View>);
                    true
                }
                _ => false,
            }
        }

        fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

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

    #[test]
    fn test_ota_view_consumes_own_focus_event() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let (hub, _rx) = channel();
        let mut bus: Bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let focus_evt = Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput)));
        let handled = ota.handle_event(&focus_evt, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            handled,
            "OtaView must consume focus events for its own ViewIds"
        );
        assert!(bus.is_empty(), "Focus event must not leak to parent bus");
    }

    #[test]
    fn test_ota_view_does_not_consume_foreign_focus_event() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let (hub, _rx) = channel();
        let mut bus: Bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let focus_evt = Event::Focus(Some(ViewId::HomeSearchInput));
        let handled = ota.handle_event(&focus_evt, &hub, &mut bus, &mut rq, &mut context);

        assert!(
            !handled,
            "OtaView must not consume focus events for other ViewIds"
        );
    }

    /// Simulates the full event dispatch chain when OtaView shows the PR
    /// input screen.
    ///
    /// The `Event::Show` handler sends `Event::Focus(Some(Ota(PrInput)))`
    /// to the hub. We drain the hub and dispatch each event through the
    /// view tree — just like the main loop does — and assert that the
    /// parent never inserts a keyboard child.
    #[test]
    fn test_parent_keyboard_not_shown_when_ota_focuses_input() {
        let mut context = create_test_context();
        context.load_keyboard_layouts();
        context.load_dictionaries();
        let (hub, rx) = channel();
        let mut bus: Bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        let mut parent = FakeParentView::new(rect![0, 0, 600, 800]);
        let ota = create_ota_view(&mut context);
        parent.children.push(Box::new(ota) as Box<dyn View>);

        assert!(
            !parent.has_keyboard(),
            "Parent must not have keyboard before focus"
        );

        let show_evt = Event::Show(ViewId::Ota(OtaViewId::PrInput));
        handle_event(
            &mut parent,
            &show_evt,
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        while let Ok(evt) = rx.try_recv() {
            handle_event(&mut parent, &evt, &hub, &mut bus, &mut rq, &mut context);
        }

        assert!(
            !parent.has_keyboard(),
            "Parent keyboard must not be shown — OtaView should consume its own focus event"
        );
    }
}
