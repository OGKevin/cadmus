use super::device_auth::DeviceAuthView;
use super::dialog::Dialog;
use super::input_field::InputField;
use super::label::Label;
use super::notification::Notification;
use super::progress_bar::ProgressBar;
use super::toggleable_keyboard::ToggleableKeyboard;
use super::{
    Align, Bus, EntryId, Event, Hub, Id, NotificationEvent, RenderData, RenderQueue, UpdateMode,
    View, ViewId, ID_FEEDER,
};
use crate::color::WHITE;
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::fl;
use crate::font::{font_from_style, Fonts, NORMAL_STYLE};
use crate::framebuffer::Framebuffer;
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::github::device_flow;
use crate::github::GithubClient;
use crate::ota::{OtaClient, OtaError, OtaProgress};
use crate::unit::scale_by_dpi;
use crate::version::{get_current_version, VersionComparison};
use crate::view::filler::Filler;
use crate::view::github::GithubEvent;
use crate::view::BIG_BAR_HEIGHT;
use secrecy::SecretString;
use std::thread;
use tracing::{error, info};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum OtaViewId {
    Main,
    SourceSelection,
    PrInput,
    DeviceAuth,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum OtaEntryId {
    DefaultBranch,
    StableRelease,
}

/// Attempts to show the OTA update view with validation checks.
///
/// This function validates prerequisites before showing the OTA view:
/// - Checks if WiFi is enabled
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

    if !context.online {
        let notif = Notification::new(
            None,
            fl!("notification-not-online"),
            false,
            hub,
            rq,
            context,
        );
        view.children_mut().push(Box::new(notif) as Box<dyn View>);
        return false;
    }

    let ota_view = OtaView::new(context);
    view.children_mut()
        .push(Box::new(ota_view) as Box<dyn View>);
    true
}

/// Which download to resume after device flow authentication completes.
#[derive(Debug, Clone)]
enum PendingDownload {
    DefaultBranch,
    PrInputPending,
    Pr(u32),
}

/// UI view for downloading and installing OTA updates from GitHub.
///
/// Manages two screens:
/// 1. Source selection dialog - asks where to download from
///    (Stable Release, Main Branch, or PR Build)
/// 2. PR input screen - prompts for PR number input (only for PR Build)
///
/// Once a download starts, the view transitions to a full-screen progress
/// screen showing a status label and a [`ProgressBar`]. On successful
/// deployment the label updates to "Rebooting…" and the app reboots
/// automatically via [`Event::Select`] with [`EntryId::Reboot`].
///
/// When a GitHub token is required but not present, the view pushes a
/// [`DeviceAuthView`] child to guide the user through device flow
/// authentication. Once authorized, the pending download resumes automatically.
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
    github_token: Option<SecretString>,
    keyboard_index: Option<usize>,
    pending_download: Option<PendingDownload>,
    /// Index into `children` of the status `Label` shown during download.
    status_label_index: Option<usize>,
    /// Index into `children` of the `ProgressBar` shown during download.
    progress_bar_index: Option<usize>,
}

impl OtaView {
    /// Creates a new OTA view.
    ///
    /// Attempts to load a previously saved GitHub token from disk. If none is
    /// found the view will prompt for device flow authentication when a
    /// token-gated download is requested.
    ///
    /// Initially displays the source selection dialog asking where to
    /// download updates from.
    ///
    /// # Arguments
    ///
    /// * `context` - Application context containing fonts and device information
    #[cfg_attr(feature = "otel", tracing::instrument(skip_all))]
    pub fn new(context: &mut Context) -> OtaView {
        let id = ID_FEEDER.next();
        let view_id = ViewId::Ota(OtaViewId::Main);
        let (width, height) = CURRENT_DEVICE.dims;

        let github_token = match device_flow::load_token() {
            Ok(token) => token,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load saved GitHub token");
                None
            }
        };

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
            pending_download: None,
            status_label_index: None,
            progress_bar_index: None,
        }
    }

    /// Builds the source selection dialog.
    #[inline]
    fn build_source_selection_dialog(context: &mut Context) -> Dialog {
        let builder = Dialog::builder(
            ViewId::Ota(OtaViewId::Main),
            "Where to check for updates?".to_string(),
        );

        #[cfg(not(feature = "test"))]
        let mut builder = builder;

        #[cfg(not(feature = "test"))]
        {
            builder = builder.add_button(
                "Stable Release",
                Event::Select(EntryId::Ota(OtaEntryId::StableRelease)),
            );
        }

        builder
            .add_button(
                "Main Branch",
                Event::Select(EntryId::Ota(OtaEntryId::DefaultBranch)),
            )
            .add_button("PR Build", Event::Show(ViewId::Ota(OtaViewId::PrInput)))
            .build(context)
    }

    /// Builds the PR input screen with title, input field, and keyboard.
    fn build_pr_input_screen(&mut self, context: &mut Context) {
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = CURRENT_DEVICE.dims;

        self.children.clear();
        self.status_label_index = None;
        self.progress_bar_index = None;
        self.keyboard_index = None;

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

    /// Builds the full-screen progress screen shown during download/deployment.
    ///
    /// Clears all existing children and adds:
    /// 1. A white full-screen [`Filler`] background
    /// 2. A centered [`Label`] with the given status text
    /// 3. A centered [`ProgressBar`] below the label
    ///
    /// The indices of the label and progress bar are stored so they can be
    /// updated incrementally as progress events arrive.
    fn build_progress_screen(&mut self, status: &str, context: &mut Context) {
        let dpi = CURRENT_DEVICE.dpi;
        let (width, height) = CURRENT_DEVICE.dims;

        self.children.clear();
        self.status_label_index = None;
        self.progress_bar_index = None;
        self.keyboard_index = None;

        self.children.push(Box::new(Filler::new(
            rect![0, 0, width as i32, height as i32],
            WHITE,
        )));

        let font = font_from_style(&mut context.fonts, &NORMAL_STYLE, dpi);
        let label_height = font.x_heights.0 as i32 * 3;
        let bar_height = scale_by_dpi(40.0, dpi) as i32;
        let bar_width = (width as f32 * 0.6) as i32;
        let center_y = height as i32 / 2;
        let gap = scale_by_dpi(24.0, dpi) as i32;

        let label_rect = rect![
            0,
            center_y - label_height - gap / 2,
            width as i32,
            center_y - gap / 2
        ];
        self.children.push(Box::new(Label::new(
            label_rect,
            status.to_string(),
            Align::Center,
        )));
        self.status_label_index = Some(self.children.len() - 1);

        let bar_x = (width as i32 - bar_width) / 2;
        let bar_rect = rect![
            bar_x,
            center_y + gap / 2,
            bar_x + bar_width,
            center_y + gap / 2 + bar_height
        ];
        self.children.push(Box::new(ProgressBar::new(bar_rect, 0)));
        self.progress_bar_index = Some(self.children.len() - 1);

        self.rect = rect![0, 0, width as i32, height as i32];
    }

    /// Toggles keyboard visibility based on focus state.
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
    /// Validates the input, transitions to the progress screen, and initiates
    /// the download. The view stays alive so it can receive progress events and
    /// handle token-invalid errors.
    fn handle_pr_submission(
        &mut self,
        text: &str,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        if let Ok(pr_number) = text.trim().parse::<u32>() {
            self.pending_download = Some(PendingDownload::Pr(pr_number));
            self.build_progress_screen(&format!("Downloading PR #{} build…", pr_number), context);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
            self.start_pr_download(pr_number, hub);
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

    /// Checks that a GitHub token is available.
    ///
    /// Returns `true` if a token is present and the caller may proceed.
    /// If no token is found, pushes a [`DeviceAuthView`] child to guide the
    /// user through device flow authentication and returns `false`.
    fn require_github_token(
        &mut self,
        pending: PendingDownload,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if self.github_token.is_some() {
            return true;
        }

        tracing::info!("No GitHub token found, starting device flow");
        self.pending_download = Some(pending);
        let auth_view = DeviceAuthView::new(hub, context);
        self.children.push(Box::new(auth_view) as Box<dyn View>);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        false
    }

    /// Initiates the PR artifact download in a background thread.
    ///
    /// Sends [`Event::OtaDownloadProgress`] during the download. On success,
    /// updates the status label to "Rebooting…" and sends
    /// [`Event::Select`] with [`EntryId::Reboot`] to trigger an automatic reboot.
    /// On a 401 response, sends [`Event::Github`] with [`GithubEvent::TokenInvalid`] without closing
    /// the view so re-authentication can proceed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub)))]
    fn start_pr_download(&mut self, pr_number: u32, hub: &Hub) {
        let Some(github_token) = self.github_token.clone() else {
            tracing::error!("GitHub token is missing when starting download, this code path should be unreachable due to prior validation");
            return;
        };

        let hub2 = hub.clone();
        let parent_span = tracing::Span::current();
        let ota_view_id = self.view_id;

        thread::spawn(move || {
            let _span =
                tracing::info_span!(parent: &parent_span, "pr_download_async", pr_number).entered();
            let github = match GithubClient::new(Some(github_token)) {
                Ok(c) => c,
                Err(e) => {
                    error!(error = %e, "Failed to create GitHub client");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Failed to create client: {}",
                        e
                    ))))
                    .ok();
                    return;
                }
            };
            let client = OtaClient::new(github);

            hub2.send(Event::OtaDownloadProgress {
                label: format!("Downloading PR #{} build… 0%", pr_number),
                percent: 0,
            })
            .ok();

            let download_result = client.download_pr_artifact(pr_number, |ota_progress| {
                if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                    let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                    hub2.send(Event::OtaDownloadProgress {
                        label: format!("Downloading PR #{} build… {}%", pr_number, percent),
                        percent,
                    })
                    .ok();
                }
            });

            match download_result {
                Ok(zip_path) => {
                    info!(pr_number, "Download completed, starting extraction");
                    match client.extract_and_deploy(zip_path) {
                        Ok(_) => {
                            hub2.send(Event::OtaDownloadProgress {
                                label: "Installing and rebooting…".to_string(),
                                percent: 100,
                            })
                            .ok();
                            send_reboot_after_delay(hub2.clone());
                        }
                        Err(e) => {
                            error!(error = %e, "Deployment failed");
                            hub2.send(Event::Close(ota_view_id)).ok();
                            hub2.send(Event::Notification(NotificationEvent::Show(format!(
                                "Deployment failed: {}",
                                e
                            ))))
                            .ok();
                        }
                    }
                }
                Err(OtaError::Unauthorized) | Err(OtaError::InsufficientScopes(_)) => {
                    tracing::warn!(pr_number, "GitHub token rejected — triggering re-auth");
                    hub2.send(Event::Github(GithubEvent::TokenInvalid)).ok();
                }
                Err(e) => {
                    error!(error = %e, "PR download failed");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Download failed: {}",
                        e
                    ))))
                    .ok();
                }
            }
        });
    }

    /// Initiates the default branch download in a background thread.
    ///
    /// Sends [`Event::OtaDownloadProgress`] during the download. On success,
    /// updates the status label to "Rebooting…" and sends
    /// [`Event::Select`] with [`EntryId::Reboot`] to trigger an automatic reboot.
    /// On a 401 response, sends [`Event::Github`] with [`GithubEvent::TokenInvalid`] without closing
    /// the view so re-authentication can proceed.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub)))]
    fn start_default_branch_download(&mut self, hub: &Hub) {
        let Some(github_token) = self.github_token.clone() else {
            tracing::error!("GitHub token is missing when starting download, this code path should be unreachable due to prior validation");
            return;
        };

        let hub2 = hub.clone();
        let parent_span = tracing::Span::current();
        let ota_view_id = self.view_id;

        thread::spawn(move || {
            let _span = tracing::info_span!(parent: &parent_span, "default_branch_download_async")
                .entered();
            let github = match GithubClient::new(Some(github_token)) {
                Ok(c) => c,
                Err(e) => {
                    error!(error = %e, "Failed to create GitHub client");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Failed to create client: {}",
                        e
                    ))))
                    .ok();
                    return;
                }
            };
            let client = OtaClient::new(github);

            hub2.send(Event::OtaDownloadProgress {
                label: "Downloading main branch build… 0%".to_string(),
                percent: 0,
            })
            .ok();

            let download_result = client.download_default_branch_artifact(|ota_progress| {
                if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                    let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                    hub2.send(Event::OtaDownloadProgress {
                        label: format!("Downloading main branch build… {}%", percent),
                        percent,
                    })
                    .ok();
                }
            });

            match download_result {
                Ok(zip_path) => {
                    info!("Main branch download completed, starting extraction");
                    match client.extract_and_deploy(zip_path) {
                        Ok(_) => {
                            hub2.send(Event::OtaDownloadProgress {
                                label: "Installing and rebooting…".to_string(),
                                percent: 100,
                            })
                            .ok();
                            send_reboot_after_delay(hub2.clone());
                        }
                        Err(e) => {
                            error!(error = %e, "Deployment failed");
                            hub2.send(Event::Close(ota_view_id)).ok();
                            hub2.send(Event::Notification(NotificationEvent::Show(format!(
                                "Deployment failed: {}",
                                e
                            ))))
                            .ok();
                        }
                    }
                }
                Err(OtaError::Unauthorized) | Err(OtaError::InsufficientScopes(_)) => {
                    tracing::warn!("GitHub token rejected — triggering re-auth");
                    hub2.send(Event::Github(GithubEvent::TokenInvalid)).ok();
                }
                Err(e) => {
                    error!(error = %e, "Main branch download failed");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Download failed: {}",
                        e
                    ))))
                    .ok();
                }
            }
        });
    }

    /// Initiates the stable release download in a background thread.
    ///
    /// Sends [`Event::OtaDownloadProgress`] during the download. On success,
    /// updates the status label to "Rebooting…" and sends
    /// [`Event::Select`] with [`EntryId::Reboot`] to trigger an automatic reboot.
    /// On a 401 response, sends [`Event::Github`] with [`GithubEvent::TokenInvalid`] without closing
    /// the view so re-authentication can proceed.
    ///
    /// GitHub authentication is not required for this operation.
    ///
    /// # Arguments
    ///
    /// * `hub` - Event hub for sending notifications and status updates
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub)))]
    fn start_stable_release_download(&mut self, hub: &Hub) {
        let github_token = self.github_token.clone();
        let hub2 = hub.clone();
        let parent_span = tracing::Span::current();
        let ota_view_id = self.view_id;

        thread::spawn(move || {
            let _span = tracing::info_span!(parent: &parent_span, "stable_release_download_async")
                .entered();
            let github = match GithubClient::new(github_token) {
                Ok(c) => c,
                Err(e) => {
                    error!(error = %e, "Failed to create GitHub client");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Failed to create client: {}",
                        e
                    ))))
                    .ok();
                    return;
                }
            };
            let client = OtaClient::new(github);

            hub2.send(Event::OtaDownloadProgress {
                label: "Downloading stable release… 0%".to_string(),
                percent: 0,
            })
            .ok();

            let download_result = client.download_stable_release_artifact(|ota_progress| {
                if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                    let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                    hub2.send(Event::OtaDownloadProgress {
                        label: format!("Downloading stable release… {}%", percent),
                        percent,
                    })
                    .ok();
                }
            });

            match download_result {
                Ok(asset_path) => {
                    info!("Stable release download completed, deploying update");
                    match client.deploy(asset_path) {
                        Ok(_) => {
                            hub2.send(Event::OtaDownloadProgress {
                                label: "Installing and rebooting…".to_string(),
                                percent: 100,
                            })
                            .ok();
                            send_reboot_after_delay(hub2.clone());
                        }
                        Err(e) => {
                            error!(error = %e, "Deployment failed");
                            hub2.send(Event::Close(ota_view_id)).ok();
                            hub2.send(Event::Notification(NotificationEvent::Show(format!(
                                "Deployment failed: {}",
                                e
                            ))))
                            .ok();
                        }
                    }
                }
                Err(OtaError::Unauthorized) | Err(OtaError::InsufficientScopes(_)) => {
                    tracing::warn!("GitHub token rejected on stable release — triggering re-auth");
                    hub2.send(Event::Github(GithubEvent::TokenInvalid)).ok();
                }
                Err(e) => {
                    error!(error = %e, "Stable release download failed");
                    hub2.send(Event::Close(ota_view_id)).ok();
                    hub2.send(Event::Notification(NotificationEvent::Show(format!(
                        "Download failed: {}",
                        e
                    ))))
                    .ok();
                }
            }
        });
    }
}

/// Spawns a thread that sleeps for 1 second then sends `Event::Select(EntryId::Reboot)`.
///
/// The delay gives the render loop time to process the final
/// `OtaDownloadProgress` label update before the event loop exits.
fn send_reboot_after_delay(hub: Hub) {
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(1));
        hub.send(Event::Select(EntryId::Reboot)).ok();
    });
}

impl OtaView {
    #[inline]
    fn on_select_default_branch(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        if !self.require_github_token(PendingDownload::DefaultBranch, hub, rq, context) {
            return true;
        }
        self.pending_download = Some(PendingDownload::DefaultBranch);
        self.build_progress_screen("Downloading main branch build… 0%", context);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
        self.start_default_branch_download(hub);
        true
    }

    #[inline]
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, hub)))]
    fn on_select_stable_release(&mut self, hub: &Hub) -> bool {
        let github_token = self.github_token.clone();
        let ota_view_id = self.view_id;

        let github = match GithubClient::new(github_token) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create GitHub client");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(format!(
                    "Failed to create client: {}",
                    e
                ))))
                .ok();
                return true;
            }
        };

        let client = OtaClient::new(github);
        let remote_version = match client.fetch_latest_release_version() {
            Ok(version) => version,
            Err(e) => {
                tracing::error!(error = %e, "Failed to fetch or parse latest release version");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(format!(
                    "Failed to check for updates: {}",
                    e
                ))))
                .ok();
                return true;
            }
        };

        let current_version = get_current_version();

        tracing::info!(
            current_version = %current_version,
            remote_version = %remote_version,
            "Comparing versions"
        );

        match current_version.compare(&remote_version) {
            Ok(VersionComparison::Equal) => {
                tracing::info!("Current version equals remote version - already latest");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(
                    "You already have the latest version".to_string(),
                )))
                .ok();
            }
            Ok(VersionComparison::Newer) => {
                tracing::info!("Current version is newer than remote version");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(
                    "Your version is newer than the latest release".to_string(),
                )))
                .ok();
            }
            Ok(VersionComparison::Older) => {
                tracing::info!("Remote version is newer - proceeding with download");
                hub.send(Event::StartStableReleaseDownload).ok();
            }
            Ok(VersionComparison::Incomparable) => {
                tracing::warn!("Cannot compare versions - divergent branches");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(
                    "Cannot compare versions - divergent branches".to_string(),
                )))
                .ok();
            }
            Err(e) => {
                tracing::error!(error = %e, "Version comparison error");
                hub.send(Event::Close(ota_view_id)).ok();
                hub.send(Event::Notification(NotificationEvent::Show(format!(
                    "Version comparison error: {}",
                    e
                ))))
                .ok();
            }
        }

        true
    }

    #[inline]
    fn on_show_pr_input(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut Context) -> bool {
        if !self.require_github_token(PendingDownload::PrInputPending, hub, rq, context) {
            return true;
        }
        self.build_pr_input_screen(context);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        self.toggle_keyboard(true, hub, rq, context);
        hub.send(Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput))))
            .ok();
        true
    }

    #[inline]
    fn on_download_progress(&mut self, label: &str, percent: u8, rq: &mut RenderQueue) -> bool {
        if let Some(idx) = self.status_label_index {
            if let Some(child) = self.children.get_mut(idx) {
                if let Some(lbl) = child.downcast_mut::<Label>() {
                    lbl.update(label, rq);
                }
            }
        }

        if percent == 100 {
            if let Some(idx) = self.progress_bar_index.take() {
                let bar_rect = *self.children[idx].rect();
                self.children.remove(idx);
                rq.add(RenderData::expose(bar_rect, UpdateMode::Gui));
            }
        } else if let Some(idx) = self.progress_bar_index {
            if let Some(child) = self.children.get_mut(idx) {
                if let Some(bar) = child.downcast_mut::<ProgressBar>() {
                    bar.update(percent, rq);
                }
            }
        }

        true
    }

    #[inline]
    fn on_device_auth_complete(
        &mut self,
        token: &secrecy::SecretString,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        tracing::info!("Device auth complete, saving token");

        if let Err(e) = device_flow::save_token(token) {
            tracing::error!(error = %e, "Failed to save GitHub token");
        }

        self.github_token = Some(token.clone());

        match self.pending_download.take() {
            Some(PendingDownload::DefaultBranch) => {
                self.build_progress_screen("Downloading main branch build… 0%", context);
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_default_branch_download(hub);
            }
            Some(PendingDownload::PrInputPending) => {
                self.build_pr_input_screen(context);
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                self.toggle_keyboard(true, hub, rq, context);
                hub.send(Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput))))
                    .ok();
            }
            Some(PendingDownload::Pr(pr_number)) => {
                self.build_progress_screen(
                    &format!("Downloading PR #{} build… 0%", pr_number),
                    context,
                );
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_pr_download(pr_number, hub);
            }
            None => {}
        }

        true
    }

    #[inline]
    fn on_token_invalid(&mut self, hub: &Hub, rq: &mut RenderQueue, context: &mut Context) -> bool {
        tracing::warn!("Saved GitHub token is invalid — clearing and re-authenticating");

        if let Err(e) = device_flow::delete_token() {
            tracing::error!(error = %e, "Failed to delete stale token");
        }

        self.github_token = None;

        let auth_view = DeviceAuthView::new(hub, context);
        self.children.push(Box::new(auth_view) as Box<dyn View>);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn on_device_auth_expired(&mut self, hub: &Hub) -> bool {
        tracing::warn!("Device flow code expired");
        self.pending_download = None;
        hub.send(Event::Notification(NotificationEvent::Show(
            "GitHub authorization timed out. Please try again.".to_string(),
        )))
        .ok();
        hub.send(Event::Close(self.view_id)).ok();
        true
    }

    #[inline]
    fn on_device_auth_error(&mut self, msg: &str, hub: &Hub) -> bool {
        tracing::error!(error = %msg, "Device flow error");
        self.pending_download = None;
        hub.send(Event::Notification(NotificationEvent::Show(format!(
            "GitHub auth error: {}",
            msg
        ))))
        .ok();
        hub.send(Event::Close(self.view_id)).ok();
        true
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
        match evt {
            Event::Select(EntryId::Ota(OtaEntryId::DefaultBranch)) => {
                self.on_select_default_branch(hub, rq, context)
            }
            Event::Select(EntryId::Ota(OtaEntryId::StableRelease)) => {
                self.on_select_stable_release(hub)
            }
            Event::Show(ViewId::Ota(OtaViewId::PrInput)) => self.on_show_pr_input(hub, rq, context),
            Event::Focus(None) => {
                self.toggle_keyboard(false, hub, rq, context);
                true
            }
            Event::Focus(Some(ViewId::Ota(_))) => true,
            Event::Submit(ViewId::Ota(OtaViewId::PrInput), ref text) => {
                self.toggle_keyboard(false, hub, rq, context);
                let text = text.clone();
                self.handle_pr_submission(&text, hub, rq, context);
                true
            }
            Event::Gesture(GestureEvent::Tap(center)) => {
                self.handle_outside_tap(*center, context, hub);
                true
            }
            Event::OtaDownloadProgress { label, percent } => {
                self.on_download_progress(label, *percent, rq)
            }
            Event::Github(GithubEvent::DeviceAuthComplete(ref token)) => {
                self.on_device_auth_complete(token, hub, rq, context)
            }
            Event::Github(GithubEvent::TokenInvalid) => self.on_token_invalid(hub, rq, context),
            Event::Github(GithubEvent::DeviceAuthExpired) => self.on_device_auth_expired(hub),
            Event::Github(GithubEvent::DeviceAuthError(ref msg)) => {
                self.on_device_auth_error(msg, hub)
            }
            Event::StartStableReleaseDownload => {
                self.build_progress_screen("Downloading stable release… 0%", context);
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_stable_release_download(hub);
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
        OtaView::new(context)
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
        crate::crypto::init_crypto_provider();

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
