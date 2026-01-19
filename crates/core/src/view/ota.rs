use super::input_field::InputField;
use super::label::Label;
use super::notification::Notification;
use super::toggleable_keyboard::ToggleableKeyboard;
use super::{Align, Bus, Event, Hub, Id, RenderQueue, View, ViewId, ID_FEEDER};
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
use std::thread;

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
pub fn show_ota_view(
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut Context,
) -> bool {
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
/// Provides an interactive interface where users can enter a PR number to
/// download and install the associated build artifact. The view includes:
/// - Title label explaining the purpose
/// - Input field for entering PR number
/// - On-screen keyboard for text entry
///
/// Download and deployment happens asynchronously in a background thread.
pub struct OtaView {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    view_id: ViewId,
    github_token: String,
    keyboard_index: usize,
}

impl OtaView {
    /// Creates a new OTA view with the configured GitHub token.
    ///
    /// Sets up the UI layout including title, input field, and keyboard.
    /// The view is automatically sized and positioned based on the device
    /// screen dimensions.
    ///
    /// # Arguments
    ///
    /// * `github_token` - GitHub personal access token for API authentication
    /// * `context` - Application context containing fonts and device information
    pub fn new(github_token: String, context: &mut Context) -> OtaView {
        let id = ID_FEEDER.next();
        let view_id = ViewId::OtaView;
        let mut children: Vec<Box<dyn View>> = Vec::new();
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = CURRENT_DEVICE.dims;

        children.push(Box::new(Filler::new(
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
        children.push(Box::new(title));

        let input_rect = rect![
            rect.min.x + 2 * padding,
            rect.min.y + padding + 4 * x_height,
            rect.max.x - 2 * padding,
            rect.min.y + padding + 8 * x_height
        ];
        let input = InputField::new(input_rect, ViewId::OtaPrInput);
        children.push(Box::new(input));

        let screen_rect = rect![0, 0, width as i32, height as i32];
        let keyboard = ToggleableKeyboard::new(screen_rect, true);
        children.push(Box::new(keyboard));
        let keyboard_index = children.len() - 1;

        OtaView {
            id,
            rect,
            children,
            view_id,
            github_token,
            keyboard_index,
        }
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
        if let Some(keyboard) = self.children.get_mut(self.keyboard_index) {
            if let Some(kb) = keyboard.downcast_mut::<ToggleableKeyboard>() {
                kb.set_visible(visible, hub, rq, context);
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
            hub.send(Event::Notify(format!(
                "Downloading PR #{} build...",
                pr_number
            )))
            .ok();
            self.start_download(pr_number, hub);
            hub.send(Event::Close(self.view_id)).ok();
        } else {
            hub.send(Event::Notify("Invalid PR number".to_string()))
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
    fn start_download(&mut self, pr_number: u32, hub: &Hub) {
        let github_token = self.github_token.clone();
        let hub2 = hub.clone();

        thread::spawn(move || {
            let client = match OtaClient::new(github_token) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[OTA] Failed to create github client {:?}", e);
                    let error_msg = format!("Failed to create client: {}", e);
                    hub2.send(Event::Notify(error_msg)).ok();
                    return;
                }
            };

            let notify_id = ViewId::MessageNotif(ID_FEEDER.next());
            hub2.send(Event::PinnedNotify(
                notify_id,
                "Starting update download".to_string(),
            ))
            .ok();
            hub2.send(Event::UpdateNotificationProgress(notify_id, 0))
                .ok();

            let download_result =
                client.download_pr_artifact(pr_number, |progress| match progress {
                    OtaProgress::DownloadingArtifact { downloaded, total } => {
                        let progress = (downloaded as f32 / total as f32) * 100.0;
                        let msg = format!("Downloading update: {}%", progress as u8);
                        hub2.send(Event::UpdateNotification(notify_id, msg)).ok();
                        hub2.send(Event::UpdateNotificationProgress(notify_id, progress as u8))
                            .ok();
                    }
                    _ => { /* Ignore other progress updates for now */ }
                });

            hub2.send(Event::Close(notify_id)).ok();

            match download_result {
                Ok(zip_path) => {
                    println!("[OTA] Download completed, starting extraction...");

                    match client.extract_and_deploy(zip_path) {
                        Ok(_) => {
                            hub2.send(Event::Notify(
                                "Update installed! Reboot to apply.".to_string(),
                            ))
                            .ok();
                        }
                        Err(e) => {
                            println!("[OTA] Deployment error: {:?}", e);
                            let error_msg = format!("Deployment failed: {}", e);
                            hub2.send(Event::Notify(error_msg)).ok();
                        }
                    }
                }
                Err(e) => {
                    println!("[OTA] Download error: {:?}", e);
                    let error_msg = format!("Download failed: {}", e);
                    hub2.send(Event::Notify(error_msg)).ok();
                }
            }
        });
    }
}

impl View for OtaView {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Focus(Some(ViewId::OtaPrInput)) => {
                self.toggle_keyboard(true, hub, rq, context);
                true
            }
            Event::Focus(None) => {
                self.toggle_keyboard(false, hub, rq, context);
                true
            }
            Event::Submit(ViewId::OtaPrInput, ref text) => {
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
