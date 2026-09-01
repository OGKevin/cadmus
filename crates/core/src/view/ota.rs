use super::button::Button;
use super::device_auth::DeviceAuthView;
use super::dialog::Dialog;
use super::input_field::InputField;
use super::label::Label;
use super::notification::Notification;
use super::progress_bar::ProgressBar;
use super::toggleable_keyboard::ToggleableKeyboard;
use super::{
    Align, Bus, EntryId, Event, Hub, ID_FEEDER, Id, NotificationEvent, RenderData, RenderQueue,
    UpdateMode, View, ViewId,
};
use crate::color::WHITE;
use crate::device::AppContext;
use crate::device::inhibitor::{Inhibitor, InhibitorError, Kind};
use crate::device::wifi::WifiSession;
use crate::device::{DeviceIdentity as _, DevicePaths as _};
use crate::fl;
use crate::font::{NORMAL_STYLE, font_from_style};
use crate::geom::Rectangle;
use crate::gesture::GestureEvent;
use crate::github::GithubClient;
use crate::github::device_flow;
use crate::ota::{
    CancelFlag, CancelFunc, DeployOutcome, OtaClient, OtaError, OtaProgress, clean_bundled_files,
    cleanup_ota_cancel,
};
use crate::unit::scale_by_dpi;
use crate::version::{VersionComparison, get_current_version};
use crate::view::BIG_BAR_HEIGHT;
use crate::view::filler::Filler;
use crate::view::github::GithubEvent;
use secrecy::SecretString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    feature = "tracing",
    tracing::instrument(
        skip_all, ret(level=tracing::Level::TRACE),
        ret(level = tracing::Level::TRACE)
    )
)]
pub fn show_ota_view(
    view: &mut dyn View,
    hub: &Hub,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) -> bool {
    #[cfg(feature = "tracing")]
    tracing::trace!("showing ota view");

    if !context.online && !context.settings.wifi.allows_on_demand() {
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
    StableReleaseCheck,
    StableReleaseDownload,
}

#[derive(Clone, Copy)]
enum OtaDownloadKind {
    /// GitHub Actions artifact from a pull request build.
    Pr(u32),
    /// Latest artifact from the repository default branch.
    DefaultBranch,
    /// Latest stable release asset.
    StableRelease,
}

impl OtaDownloadKind {
    fn progress_label(self, percent: u8) -> String {
        match self {
            Self::Pr(pr_number) => fl!(
                "ota-downloading-pr",
                pr_number = pr_number,
                percent = percent
            ),
            Self::DefaultBranch => fl!("ota-downloading-default-branch", percent = percent),
            Self::StableRelease => fl!("ota-downloading-stable-release", percent = percent),
        }
    }
}

/// UI view for downloading and installing OTA updates from GitHub.
///
/// Manages two screens:
/// 1. Source selection dialog - asks where to download from
///    (Stable Release, Main Branch, or PR Build)
/// 2. PR input screen - prompts for PR number input (only for PR Build)
///
/// Once a download starts, the view transitions to a full-screen progress
/// screen showing a status label, a [`ProgressBar`], and a Cancel button.
/// Downloads run in a background thread via [`run_ota_download`]. On successful
/// deployment the label updates to "Installing and rebooting…" and the app
/// reboots automatically via [`Event::Select`] with [`EntryId::Reboot`].
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
    auth: device_flow::ResolvedAuth,
    keyboard_index: Option<usize>,
    pending_download: Option<PendingDownload>,
    /// Index into `children` of the status `Label` shown during download.
    status_label_index: Option<usize>,
    /// Index into `children` of the `ProgressBar` shown during download.
    progress_bar_index: Option<usize>,
    /// Index into `children` of the Cancel button shown during download.
    cancel_button_index: Option<usize>,
    cancelled: Arc<CancelFlag>,
    download_in_progress: bool,
    download_committed: bool,
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
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn new(context: &mut AppContext) -> OtaView {
        let id = ID_FEEDER.next();
        let view_id = ViewId::Ota(OtaViewId::Main);
        let (width, height) = context.device.dims();

        let auth = match device_flow::ResolvedAuth::load(&context.device.install_dir()) {
            Ok(auth) => auth,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load saved GitHub token");
                device_flow::ResolvedAuth::empty()
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
            auth,
            keyboard_index: None,
            pending_download: None,
            status_label_index: None,
            progress_bar_index: None,
            cancel_button_index: None,
            cancelled: Arc::new(CancelFlag::new()),
            download_in_progress: false,
            download_committed: false,
        }
    }

    /// Builds the source selection dialog.
    #[inline]
    fn build_source_selection_dialog(context: &mut AppContext) -> Dialog {
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
    fn build_pr_input_screen(&mut self, context: &mut AppContext) {
        let dpi = context.device.dpi();
        let (width, height) = context.device.dims();

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
        let title = Label::new(title_rect, fl!("ota-pr-input-title"), Align::Center);
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
    /// 4. A Cancel [`Button`] below the progress bar
    ///
    /// The indices of the label, progress bar, and cancel button are stored so
    /// they can be updated incrementally as progress events arrive.
    fn build_progress_screen(&mut self, status: &str, context: &mut AppContext) {
        let dpi = context.device.dpi();
        let (width, height) = context.device.dims();

        self.children.clear();
        self.status_label_index = None;
        self.progress_bar_index = None;
        self.cancel_button_index = None;
        self.keyboard_index = None;
        self.download_committed = false;

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

        let button_width = scale_by_dpi(200.0, dpi) as i32;
        let button_height = scale_by_dpi(40.0, dpi) as i32;
        let button_x = (width as i32 - button_width) / 2;
        let button_y = center_y + gap / 2 + bar_height + gap;
        let cancel_rect = rect![
            button_x,
            button_y,
            button_x + button_width,
            button_y + button_height
        ];
        self.children.push(Box::new(Button::new(
            cancel_rect,
            Event::Close(self.view_id),
            fl!("ota-download-cancel"),
        )));
        self.cancel_button_index = Some(self.children.len() - 1);

        self.rect = rect![0, 0, width as i32, height as i32];
    }

    /// Resets cancel state and marks a new download as in progress.
    fn begin_download(&mut self) {
        self.cancelled = Arc::new(CancelFlag::new());
        self.download_in_progress = true;
        self.download_committed = false;
    }

    /// Signals the background download thread to stop and clean up.
    fn cancel_download(&self) {
        self.cancelled.request_cancel();
    }

    /// Removes the Cancel button after the update is committed to disk.
    fn hide_cancel_button(&mut self, rq: &mut RenderQueue) {
        if let Some(idx) = self.cancel_button_index.take()
            && let Some(child) = self.children.get(idx)
        {
            let button_rect = *child.rect();
            self.children.remove(idx);
            self.shift_child_indices_after_remove(idx);
            rq.add(RenderData::expose(button_rect, UpdateMode::Gui));
        }
    }

    fn shift_child_indices_after_remove(&mut self, removed: usize) {
        for index in [
            &mut self.status_label_index,
            &mut self.progress_bar_index,
            &mut self.cancel_button_index,
        ] {
            if let Some(i) = index
                && *i > removed
            {
                *i -= 1;
            }
        }
    }

    /// Toggles keyboard visibility based on focus state.
    fn toggle_keyboard(
        &mut self,
        visible: bool,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
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
        context: &mut AppContext,
    ) {
        if let Ok(pr_number) = text.trim().parse::<u32>() {
            self.pending_download = Some(PendingDownload::Pr(pr_number));
            self.build_progress_screen(&OtaDownloadKind::Pr(pr_number).progress_label(0), context);
            rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
            self.start_pr_download(pr_number, hub, context);
        } else {
            hub.send(
                (Event::Notification(NotificationEvent::Show(fl!("ota-invalid-pr-number")))).into(),
            )
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
    fn handle_outside_tap(
        &self,
        tap_position: crate::geom::Point,
        context: &AppContext,
        hub: &Hub,
    ) {
        if !self.rect.includes(tap_position)
            && !context.kb_rect.includes(tap_position)
            && !context.kb_rect.is_empty()
        {
            if self.download_in_progress && !self.download_committed {
                self.cancel_download();
            } else {
                hub.send((Event::Close(self.view_id)).into()).ok();
            }
        }
    }

    /// Returns the GitHub token used for OTA, including a debug-build
    /// `GH_TOKEN` environment override when present and not suppressed.
    fn effective_github_token(&self) -> Option<SecretString> {
        self.auth.effective()
    }

    /// Restarts the in-flight download with the currently effective token.
    fn resume_pending_download(&mut self, hub: &Hub, context: &mut AppContext) {
        match self.pending_download.clone() {
            Some(PendingDownload::DefaultBranch) => {
                self.start_default_branch_download(hub, context);
            }
            Some(PendingDownload::Pr(pr_number)) => {
                self.start_pr_download(pr_number, hub, context);
            }
            Some(PendingDownload::StableReleaseCheck) => {
                self.on_select_stable_release(hub, context);
            }
            Some(PendingDownload::StableReleaseDownload) => {
                self.start_stable_release_download(hub, context);
            }
            Some(PendingDownload::PrInputPending) | None => {}
        }
    }

    /// Consumes a close request during an in-flight download by cancelling instead
    /// of closing immediately; the background thread closes the view after cleanup.
    fn on_close_during_download(&mut self) -> bool {
        if self.download_in_progress && !self.download_committed {
            self.cancel_download();
            true
        } else {
            false
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
        context: &mut AppContext,
    ) -> bool {
        if self.effective_github_token().is_some() {
            return true;
        }

        tracing::info!("No GitHub token found, starting device flow");
        self.pending_download = Some(pending);
        let auth_view = DeviceAuthView::new(hub, context);
        self.children.push(Box::new(auth_view) as Box<dyn View>);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        false
    }

    /// Starts a PR artifact download via [`run_ota_download`].
    ///
    /// Requires a GitHub token; callers must validate with
    /// [`Self::require_github_token`] first.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, context)))]
    fn start_pr_download(&mut self, pr_number: u32, hub: &Hub, context: &mut AppContext) {
        let Some(github_token) = self.effective_github_token() else {
            tracing::error!(
                "GitHub token is missing when starting download, this code path should be unreachable due to prior validation"
            );
            return;
        };

        self.begin_download();
        run_ota_download(OtaDownloadContext {
            kind: OtaDownloadKind::Pr(pr_number),
            hub: hub.clone(),
            ota_view_id: self.view_id,
            tmp_dir: context.device.tmp_dir(),
            install_dir: context.device.install_dir(),
            github_token: Some(github_token),
            cancelled: Arc::clone(&self.cancelled),
            wifi_session: context.wifi_session.clone(),
            inhibitor: Arc::clone(&context.inhibitor),
        });
    }

    /// Starts a default-branch artifact download via [`run_ota_download`].
    ///
    /// Requires a GitHub token; callers must validate with
    /// [`Self::require_github_token`] first.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, context)))]
    fn start_default_branch_download(&mut self, hub: &Hub, context: &mut AppContext) {
        let Some(github_token) = self.effective_github_token() else {
            tracing::error!(
                "GitHub token is missing when starting download, this code path should be unreachable due to prior validation"
            );
            return;
        };

        self.begin_download();
        run_ota_download(OtaDownloadContext {
            kind: OtaDownloadKind::DefaultBranch,
            hub: hub.clone(),
            ota_view_id: self.view_id,
            tmp_dir: context.device.tmp_dir(),
            install_dir: context.device.install_dir(),
            github_token: Some(github_token),
            cancelled: Arc::clone(&self.cancelled),
            wifi_session: context.wifi_session.clone(),
            inhibitor: Arc::clone(&context.inhibitor),
        });
    }

    /// Starts a stable release download via [`run_ota_download`].
    ///
    /// GitHub authentication is optional for this path.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, context)))]
    fn start_stable_release_download(&mut self, hub: &Hub, context: &mut AppContext) {
        self.begin_download();
        run_ota_download(OtaDownloadContext {
            kind: OtaDownloadKind::StableRelease,
            hub: hub.clone(),
            ota_view_id: self.view_id,
            tmp_dir: context.device.tmp_dir(),
            install_dir: context.device.install_dir(),
            github_token: self.effective_github_token(),
            cancelled: Arc::clone(&self.cancelled),
            wifi_session: context.wifi_session.clone(),
            inhibitor: Arc::clone(&context.inhibitor),
        });
    }
}

/// Parameters for a background OTA download started by [`run_ota_download`].
struct OtaDownloadContext {
    kind: OtaDownloadKind,
    hub: Hub,
    ota_view_id: ViewId,
    tmp_dir: PathBuf,
    install_dir: PathBuf,
    github_token: Option<SecretString>,
    cancelled: Arc<CancelFlag>,
    wifi_session: Arc<WifiSession>,
    inhibitor: Arc<Inhibitor>,
}

/// Cleans up partial OTA files and closes the view after user cancellation.
fn finish_ota_cancelled(hub: &Hub, ota_view_id: ViewId, tmp_dir: &Path, deploy_path: &Path) {
    cleanup_ota_cancel(tmp_dir, deploy_path);
    hub.send((Event::Close(ota_view_id)).into()).ok();
}

/// Completes a published OTA install, including the committed-but-not-durable
/// case where parent-directory sync failed after rename.
fn finish_successful_deploy(hub: &Hub, install_dir: &Path, outcome: DeployOutcome) {
    if let DeployOutcome::CommittedNotDurable { path, error } = &outcome {
        tracing::warn!(
            path = ?path,
            error = %error,
            "OTA bundle committed but parent directory was not synced"
        );
    }

    if let Err(e) = clean_bundled_files(install_dir) {
        tracing::warn!(path = ?install_dir, error = %e, "Failed to clean bundled OTA files");
    }
    send_ota_progress(hub, fl!("ota-installing-and-rebooting"), 100, false);
    send_reboot_after_delay(hub.clone());
}

/// Sends an [`Event::OtaDownloadProgress`] update to the UI thread.
fn send_ota_progress(hub: &Hub, label: String, percent: u8, cancelable: bool) {
    hub.send(
        (Event::OtaDownloadProgress {
            label,
            percent,
            cancelable,
        })
        .into(),
    )
    .ok();
}

/// Runs an OTA download and deployment in a background thread.
///
/// Sends [`Event::OtaDownloadProgress`] during the download and deployment.
/// Progress events set `cancelable: true` until `KoboRoot.tgz` is committed;
/// the UI hides the Cancel button when `cancelable` becomes `false`.
///
/// On success, sends a final progress update ("Installing and rebooting…") and
/// schedules an automatic reboot via [`send_reboot_after_delay`].
///
/// On user cancellation, deletes partial download and staging files via
/// [`finish_ota_cancelled`] and closes the view without rebooting.
///
/// On a 401 or insufficient-scopes response, sends [`Event::Github`] with
/// [`GithubEvent::TokenInvalid`] without closing the view so re-authentication
/// can proceed.
///
/// Acquires a WiFi lease `"ota-download"` and a Full `"ota"`
/// [`InhibitorGuard`](crate::device::inhibitor::InhibitorGuard) for the
/// download. Both drop when the thread exits (success, cancel, failure,
/// re-auth, panic). Success sends
/// [`Event::ClearDeferredSuspend`] **before** the Full guard drops, then
/// delays reboot, so a deferred Auto Suspend does not race the reboot.
fn run_ota_download(ctx: OtaDownloadContext) {
    let OtaDownloadContext {
        kind,
        hub,
        ota_view_id,
        tmp_dir,
        install_dir,
        github_token,
        cancelled,
        wifi_session,
        inhibitor,
    } = ctx;

    let hub2 = hub.clone();
    let parent_span = tracing::Span::current();
    thread::spawn(move || {
        let _span = match kind {
            OtaDownloadKind::Pr(pr_number) => tracing::info_span!(
                parent: &parent_span,
                "pr_download_async",
                pr_number
            )
            .entered(),
            OtaDownloadKind::DefaultBranch => tracing::info_span!(
                parent: &parent_span,
                "default_branch_download_async"
            )
            .entered(),
            OtaDownloadKind::StableRelease => tracing::info_span!(
                parent: &parent_span,
                "stable_release_download_async"
            )
            .entered(),
        };
        let should_cancel = CancelFunc::from_flag(&cancelled);

        let _wifi = match wifi_session.acquire("ota-download") {
            Ok(lease) => lease,
            Err(e) => {
                error!(error = %e, "Failed to acquire WiFi lease for OTA download");
                hub2.send((Event::Close(ota_view_id)).into()).ok();
                hub2.send(
                    (Event::Notification(NotificationEvent::Show(fl!("notification-not-online"))))
                        .into(),
                )
                .ok();
                return;
            }
        };

        let _full_hold = match inhibitor.acquire(Kind::Full, "ota") {
            Ok(guard) => guard,
            Err(InhibitorError::BatteryTooLow) => {
                hub2.send((Event::Close(ota_view_id)).into()).ok();
                hub2.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-battery-too-low"))))
                        .into(),
                )
                .ok();
                return;
            }
        };

        let github = match GithubClient::new(github_token) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "Failed to create GitHub client");
                hub2.send((Event::Close(ota_view_id)).into()).ok();
                hub2.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-client-build-failed"))))
                        .into(),
                )
                .ok();
                return;
            }
        };

        let client = OtaClient::new(github, tmp_dir.clone());
        let deploy_path = client.deploy_path();

        let initial_label = kind.progress_label(0);
        send_ota_progress(&hub2, initial_label, 0, true);

        let download_result = match kind {
            OtaDownloadKind::Pr(pr_number) => client.download_pr_artifact(
                pr_number,
                |ota_progress| {
                    if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                        let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                        send_ota_progress(
                            &hub2,
                            OtaDownloadKind::Pr(pr_number).progress_label(percent),
                            percent,
                            true,
                        );
                    }
                },
                should_cancel,
            ),
            OtaDownloadKind::DefaultBranch => client.download_default_branch_artifact(
                |ota_progress| {
                    if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                        let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                        send_ota_progress(
                            &hub2,
                            OtaDownloadKind::DefaultBranch.progress_label(percent),
                            percent,
                            true,
                        );
                    }
                },
                should_cancel,
            ),
            OtaDownloadKind::StableRelease => client.download_stable_release_artifact(
                |ota_progress| {
                    if let OtaProgress::DownloadingArtifact { downloaded, total } = ota_progress {
                        let percent = (downloaded as f32 / total as f32 * 100.0) as u8;
                        send_ota_progress(
                            &hub2,
                            OtaDownloadKind::StableRelease.progress_label(percent),
                            percent,
                            true,
                        );
                    }
                },
                should_cancel,
            ),
        };

        if matches!(download_result, Err(OtaError::Cancelled)) {
            finish_ota_cancelled(&hub2, ota_view_id, &tmp_dir, &deploy_path);
            return;
        }

        match download_result {
            Ok(artifact_path) => {
                info!("Download completed, starting deployment");
                let deploy_result = match kind {
                    OtaDownloadKind::StableRelease => client.deploy(artifact_path, should_cancel),
                    _ => client.extract_and_deploy(artifact_path, should_cancel),
                };

                if matches!(deploy_result, Err(OtaError::Cancelled)) {
                    finish_ota_cancelled(&hub2, ota_view_id, &tmp_dir, &deploy_path);
                    return;
                }

                match deploy_result {
                    Ok(outcome) => {
                        hub2.send((Event::ClearDeferredSuspend).into()).ok();
                        finish_successful_deploy(&hub2, &install_dir, outcome);
                    }
                    Err(e) => {
                        error!(error = %e, "Deployment failed");
                        hub2.send((Event::Close(ota_view_id)).into()).ok();
                        hub2.send(
                            (Event::Notification(NotificationEvent::Show(fl!(
                                "ota-deployment-failed"
                            ))))
                            .into(),
                        )
                        .ok();
                    }
                }
            }
            Err(OtaError::Unauthorized) | Err(OtaError::InsufficientScopes(_)) => {
                tracing::warn!("GitHub token rejected — triggering re-auth");
                hub2.send((Event::Github(GithubEvent::TokenInvalid)).into())
                    .ok();
            }
            Err(e) => {
                error!(error = %e, "OTA download failed");
                hub2.send((Event::Close(ota_view_id)).into()).ok();
                hub2.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-download-failed"))))
                        .into(),
                )
                .ok();
            }
        }
    });
}

/// Spawns a thread that sleeps for 1 second then sends `Event::Select(EntryId::Reboot)`.
///
/// The delay gives the render loop time to process the final
/// `OtaDownloadProgress` label update before the event loop exits.
fn send_reboot_after_delay(hub: Hub) {
    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_secs(1));
        hub.send((Event::Select(EntryId::Reboot)).into()).ok();
    });
}

impl OtaView {
    #[inline]
    fn on_select_default_branch(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        if !self.require_github_token(PendingDownload::DefaultBranch, hub, rq, context) {
            return true;
        }
        self.pending_download = Some(PendingDownload::DefaultBranch);
        self.build_progress_screen(&OtaDownloadKind::DefaultBranch.progress_label(0), context);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
        self.start_default_branch_download(hub, context);
        true
    }

    #[inline]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, context)))]
    fn on_select_stable_release(&mut self, hub: &Hub, context: &AppContext) -> bool {
        self.pending_download = Some(PendingDownload::StableReleaseCheck);
        let github_token = self.effective_github_token();
        let ota_view_id = self.view_id;

        let github = match GithubClient::new(github_token) {
            Ok(c) => c,
            Err(e) => {
                self.pending_download = None;
                tracing::error!(error = %e, "Failed to create GitHub client");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-client-build-failed"))))
                        .into(),
                )
                .ok();
                return true;
            }
        };

        let client = OtaClient::new(github, context.device.tmp_dir());
        let remote_version = match client.fetch_latest_release_version() {
            Ok(version) => version,
            Err(e) => {
                self.pending_download = None;
                tracing::error!(error = %e, "Failed to fetch or parse latest release version");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-check-updates-failed"))))
                        .into(),
                )
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
                self.pending_download = None;
                tracing::info!("Current version equals remote version - already latest");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-already-latest"))))
                        .into(),
                )
                .ok();
            }
            Ok(VersionComparison::Newer) => {
                self.pending_download = None;
                tracing::info!("Current version is newer than remote version");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!("ota-version-newer")))).into(),
                )
                .ok();
            }
            Ok(VersionComparison::Older) => {
                tracing::info!("Remote version is newer - proceeding with download");
                hub.send((Event::StartStableReleaseDownload).into()).ok();
            }
            Ok(VersionComparison::Incomparable) => {
                self.pending_download = None;
                tracing::warn!("Cannot compare versions - divergent branches");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!(
                        "ota-cannot-compare-versions"
                    ))))
                    .into(),
                )
                .ok();
            }
            Err(e) => {
                self.pending_download = None;
                tracing::error!(error = %e, "Version comparison error");
                hub.send((Event::Close(ota_view_id)).into()).ok();
                hub.send(
                    (Event::Notification(NotificationEvent::Show(fl!(
                        "ota-version-comparison-error"
                    ))))
                    .into(),
                )
                .ok();
            }
        }

        true
    }

    #[inline]
    fn on_show_pr_input(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        if !self.require_github_token(PendingDownload::PrInputPending, hub, rq, context) {
            return true;
        }
        self.build_pr_input_screen(context);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        self.toggle_keyboard(true, hub, rq, context);
        hub.send((Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput)))).into())
            .ok();
        true
    }

    /// Updates the progress label and bar from an [`Event::OtaDownloadProgress`].
    ///
    /// Hides the Cancel button when `cancelable` is `false`, indicating the
    /// update has been committed to disk.
    #[inline]
    fn on_download_progress(
        &mut self,
        label: &str,
        percent: u8,
        cancelable: bool,
        rq: &mut RenderQueue,
    ) -> bool {
        if !cancelable {
            self.download_committed = true;
            self.hide_cancel_button(rq);
        }

        if let Some(idx) = self.status_label_index {
            if let Some(child) = self.children.get_mut(idx) {
                if let Some(lbl) = child.downcast_mut::<Label>() {
                    lbl.update(label, rq);
                }
            }
        }

        if percent == 100 {
            if let Some(idx) = self.progress_bar_index.take()
                && let Some(child) = self.children.get(idx)
            {
                let bar_rect = *child.rect();
                self.children.remove(idx);
                self.shift_child_indices_after_remove(idx);
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
        context: &mut AppContext,
    ) -> bool {
        tracing::info!("Device auth complete, saving token");

        if let Err(e) = device_flow::save_token(token, &context.device.install_dir()) {
            tracing::error!(error = %e, "Failed to save GitHub token");
        }

        self.auth.set_saved(token.clone());

        match self.pending_download.take() {
            Some(PendingDownload::DefaultBranch) => {
                self.build_progress_screen(
                    &OtaDownloadKind::DefaultBranch.progress_label(0),
                    context,
                );
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_default_branch_download(hub, context);
            }
            Some(PendingDownload::PrInputPending) => {
                self.build_pr_input_screen(context);
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                self.toggle_keyboard(true, hub, rq, context);
                hub.send((Event::Focus(Some(ViewId::Ota(OtaViewId::PrInput)))).into())
                    .ok();
            }
            Some(PendingDownload::Pr(pr_number)) => {
                self.build_progress_screen(
                    &OtaDownloadKind::Pr(pr_number).progress_label(0),
                    context,
                );
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_pr_download(pr_number, hub, context);
            }
            Some(PendingDownload::StableReleaseCheck) => {
                self.on_select_stable_release(hub, context);
            }
            Some(PendingDownload::StableReleaseDownload) => {
                self.build_progress_screen(
                    &OtaDownloadKind::StableRelease.progress_label(0),
                    context,
                );
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.pending_download = Some(PendingDownload::StableReleaseDownload);
                self.start_stable_release_download(hub, context);
            }
            None => {}
        }

        true
    }

    #[inline]
    fn on_token_invalid(
        &mut self,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match self.auth.reject_effective() {
            Some(device_flow::AuthOrigin::Environment) => {
                tracing::warn!("GH_TOKEN rejected — ignoring it for this session");
                if self.auth.effective().is_some() {
                    self.resume_pending_download(hub, context);
                    return true;
                }
            }
            Some(device_flow::AuthOrigin::Saved) => {
                tracing::warn!("Saved GitHub token is invalid — clearing and re-authenticating");
                if let Err(e) = device_flow::delete_token(&context.device.install_dir()) {
                    tracing::error!(error = %e, "Failed to delete stale token");
                }
            }
            None => {
                tracing::warn!("GitHub token rejected with no token loaded");
            }
        }

        self.download_in_progress = false;
        self.download_committed = false;

        let auth_view = DeviceAuthView::new(hub, context);
        self.children.push(Box::new(auth_view) as Box<dyn View>);
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
    }

    #[inline]
    fn on_device_auth_expired(&mut self, hub: &Hub) -> bool {
        tracing::warn!("Device flow code expired");
        self.pending_download = None;
        hub.send((Event::Notification(NotificationEvent::Show(fl!("ota-auth-timed-out")))).into())
            .ok();
        hub.send((Event::Close(self.view_id)).into()).ok();
        true
    }

    #[inline]
    fn on_device_auth_error(&mut self, msg: &str, hub: &Hub) -> bool {
        tracing::error!(error = %msg, "Device flow error");
        self.pending_download = None;
        hub.send((Event::Notification(NotificationEvent::Show(fl!("ota-auth-error")))).into())
            .ok();
        hub.send((Event::Close(self.view_id)).into()).ok();
        true
    }
}

impl View for OtaView {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, _bus, rq, context), fields(event = ?evt
    ), ret(level=tracing::Level::TRACE)))]
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match evt {
            Event::Select(EntryId::Ota(OtaEntryId::DefaultBranch)) => {
                self.on_select_default_branch(hub, rq, context)
            }
            Event::Select(EntryId::Ota(OtaEntryId::StableRelease)) => {
                self.on_select_stable_release(hub, context)
            }
            Event::Show(ViewId::Ota(OtaViewId::PrInput)) => self.on_show_pr_input(hub, rq, context),
            Event::Focus(None) => {
                self.toggle_keyboard(false, hub, rq, context);
                true
            }
            Event::Focus(Some(ViewId::Ota(_))) => true,
            Event::Submit(ViewId::Ota(OtaViewId::PrInput), text) => {
                self.toggle_keyboard(false, hub, rq, context);
                let text = text.clone();
                self.handle_pr_submission(&text, hub, rq, context);
                true
            }
            Event::Gesture(GestureEvent::Tap(center)) => {
                self.handle_outside_tap(*center, context, hub);
                true
            }
            Event::Close(id) if *id == self.view_id => self.on_close_during_download(),
            Event::OtaDownloadProgress {
                label,
                percent,
                cancelable,
            } => self.on_download_progress(label, *percent, *cancelable, rq),
            Event::Github(GithubEvent::DeviceAuthComplete(token)) => {
                self.on_device_auth_complete(token, hub, rq, context)
            }
            Event::Github(GithubEvent::TokenInvalid) => self.on_token_invalid(hub, rq, context),
            Event::Github(GithubEvent::DeviceAuthExpired) => self.on_device_auth_expired(hub),
            Event::Github(GithubEvent::DeviceAuthError(msg)) => self.on_device_auth_error(msg, hub),
            Event::StartStableReleaseDownload => {
                self.pending_download = Some(PendingDownload::StableReleaseDownload);
                self.build_progress_screen(
                    &OtaDownloadKind::StableRelease.progress_label(0),
                    context,
                );
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
                self.start_stable_release_download(hub, context);
                true
            }
            _ => false,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, _context, _rect), fields(rect = ?_rect
    )))]
    fn render(&self, _context: &mut AppContext, _rect: Rectangle) {}

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
        _context: &mut AppContext,
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

    fn create_ota_view(context: &mut AppContext) -> OtaView {
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
            context: &mut AppContext,
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

        fn render(&self, _context: &mut AppContext, _rect: Rectangle) {}

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
    fn test_progress_screen_shows_cancel_button() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);

        let label = OtaDownloadKind::DefaultBranch.progress_label(0);
        ota.build_progress_screen(&label, &mut context);

        let cancel_idx = ota
            .cancel_button_index
            .expect("progress screen must include a cancel button");
        assert!(
            ota.children[cancel_idx].downcast_ref::<Button>().is_some(),
            "cancel child must be a Button"
        );
    }

    #[test]
    fn test_shift_child_indices_after_remove_updates_cancel_button_index() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);

        let label = OtaDownloadKind::DefaultBranch.progress_label(0);
        ota.build_progress_screen(&label, &mut context);

        assert_eq!(ota.progress_bar_index, Some(2));
        assert_eq!(ota.cancel_button_index, Some(3));

        ota.progress_bar_index = None;
        ota.shift_child_indices_after_remove(2);

        assert_eq!(ota.cancel_button_index, Some(2));
        assert_eq!(ota.status_label_index, Some(1));
    }

    #[test]
    fn test_close_during_download_sets_cancel_flag() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let (hub, _rx) = channel();
        let mut bus: Bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        ota.begin_download();
        assert!(!ota.cancelled.is_cancelled());

        let handled = ota.handle_event(
            &Event::Close(ota.view_id),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(handled);
        assert!(ota.cancelled.is_cancelled());
        assert!(ota.download_in_progress);
    }

    #[test]
    fn test_close_after_commit_does_not_cancel() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let (hub, _rx) = channel();
        let mut bus: Bus = VecDeque::new();
        let mut rq = RenderQueue::new();

        ota.begin_download();
        ota.download_committed = true;

        let handled = ota.handle_event(
            &Event::Close(ota.view_id),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        );

        assert!(!handled);
        assert!(!ota.cancelled.is_cancelled());
    }

    #[test]
    fn test_progress_non_cancelable_hides_cancel_and_marks_committed() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let mut rq = RenderQueue::new();

        let label = OtaDownloadKind::DefaultBranch.progress_label(50);
        ota.build_progress_screen(&label, &mut context);
        ota.begin_download();
        let cancel_idx = ota.cancel_button_index.expect("cancel button");

        let handled = ota.on_download_progress("Installing…", 90, false, &mut rq);

        assert!(handled);
        assert!(ota.download_committed);
        assert!(ota.cancel_button_index.is_none());
        assert!(
            ota.children
                .get(cancel_idx)
                .and_then(|c| c.downcast_ref::<Button>())
                .is_none(),
            "cancel button child must be removed"
        );
    }

    #[test]
    fn test_progress_100_removes_bar_and_shifts_cancel_index() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let mut rq = RenderQueue::new();

        let label = OtaDownloadKind::DefaultBranch.progress_label(0);
        ota.build_progress_screen(&label, &mut context);
        assert_eq!(ota.progress_bar_index, Some(2));
        assert_eq!(ota.cancel_button_index, Some(3));

        let handled = ota.on_download_progress("Done", 100, true, &mut rq);

        assert!(handled);
        assert!(ota.progress_bar_index.is_none());
        assert_eq!(ota.cancel_button_index, Some(2));
        assert!(
            ota.children[2].downcast_ref::<Button>().is_some(),
            "cancel button must remain addressable after progress bar removal"
        );
    }

    #[test]
    fn test_progress_100_then_hide_cancel_does_not_panic() {
        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        let mut rq = RenderQueue::new();

        let label = OtaDownloadKind::DefaultBranch.progress_label(0);
        ota.build_progress_screen(&label, &mut context);
        ota.on_download_progress("Done", 100, true, &mut rq);
        ota.on_download_progress("Installing…", 100, false, &mut rq);

        assert!(ota.download_committed);
        assert!(ota.cancel_button_index.is_none());
        assert!(ota.progress_bar_index.is_none());
    }

    #[test]
    fn test_effective_github_token_prefers_stored_token() {
        use secrecy::ExposeSecret;

        let mut context = create_test_context();
        let mut ota = create_ota_view(&mut context);
        ota.auth
            .set_saved(SecretString::from("stored-token".to_owned()));

        let token = ota.effective_github_token().expect("token");
        assert_eq!(token.expose_secret(), "stored-token");
    }

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

        while let Ok(message) = rx.try_recv() {
            let (evt, _) = message.into_parts();
            handle_event(&mut parent, &evt, &hub, &mut bus, &mut rq, &mut context);
        }

        assert!(
            !parent.has_keyboard(),
            "Parent keyboard must not be shown — OtaView should consume its own focus event"
        );
    }
}
