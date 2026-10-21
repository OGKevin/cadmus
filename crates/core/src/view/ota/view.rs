//! Slint screen for checking and installing an update.

use std::cell::Cell;
use std::sync::Arc;

use anyhow::Context as _;
use secrecy::SecretString;

use crate::device::AppContext;
use crate::device::{DeviceIdentity as _, DevicePaths as _};
use crate::fl;
use crate::framebuffer::UpdateMode;
use crate::geom::Rectangle;
use crate::github::GithubClient;
use crate::github::device_flow::{self, AuthOrigin};
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
use crate::ota::{CheckedPull, OpenPull, OtaClient, OtaError, pending_pulls};
#[cfg(not(feature = "test"))]
use crate::version::{VersionComparison, get_current_version};
use crate::view::github::{GithubEvent, StableReleaseCheckOutcome};
use crate::view::slint::components::choice_sheet::{self, ChoiceSheet, Row};
use crate::view::slint::components::confirm_sheet::{self, ConfirmSheet};
use crate::view::slint::generated::{OtaScreen, OtaWindow};
use crate::view::slint::{
    CallbackQueue, Component, Env, FastInk, Flow, OverlayStack, Pager, Session, SlintHost,
    SlintScreen, apply_ink, connect,
};
use crate::view::{Bus, Event, Hub, Id, NotificationEvent, RenderQueue};

use super::OtaEvent;
use super::auth::spawn_device_flow_job;
use super::auth_panel::AuthPanel;
#[cfg(not(feature = "test"))]
use super::download::DownloadOutcome;
use super::download::{OtaDownloadContext, OtaDownloadKind, run_ota_download};
use super::list::{self, List};
use super::progress::Progress;

/// Check for Updates.
pub type OtaView = SlintHost<Ota>;

/// Where an install can come from.
#[derive(Clone, Copy, Debug)]
enum Source {
    #[cfg(not(feature = "test"))]
    Stable,
    Main,
    Pr,
}

/// Work to resume after the reader signs in.
#[derive(Clone, Debug)]
enum Pending {
    Main,
    PrList,
    Pr(ResolvedPr),
}

/// A pull request the reader agreed to install.
#[derive(Clone, Copy, Debug)]
struct ResolvedPr {
    number: u32,
    artifact_id: u64,
    size_in_bytes: u64,
}

/// Which state the window is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Source,
    List,
    Auth,
    Progress,
}

/// Where to return when progress ends without a reboot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CameFrom {
    Source,
    List,
}

/// A callback from the OTA window itself. The sheets have their own queues.
#[derive(Debug)]
enum Action {
    Back,
    Line(i32),
    Refresh,
    Previous,
    Next,
    AuthCancel,
    ProgressCancel,
}

/// Check for Updates.
pub struct Ota {
    list: List,
    auth_panel: AuthPanel,
    progress: Progress,
    choice: ChoiceSheet<Source, OtaWindow>,
    confirm: ConfirmSheet<OtaWindow>,
    back: CallbackQueue<()>,
    phase: Phase,
    came_from: CameFrom,
    auth: device_flow::ResolvedAuth,
    pending: Option<Pending>,
    confirm_pr: Option<ResolvedPr>,
    pager: Pager,
    /// Every open pull request. The screen shows one slice of this.
    rows: Vec<CheckedPull>,
    list_generation: u64,
    loading: bool,
    auth_message: String,
    auth_code: String,
    progress_label: String,
    progress_percent: u8,
    progress_cancelable: bool,
    download_canceling: bool,
    cancelled: Arc<crate::http::CancelFlag>,
    download_in_progress: bool,
    download_committed: bool,
    pr_job: Option<crate::runtime::Job>,
    auth_job: Option<crate::runtime::Job>,
    auth_cancelled: Arc<crate::http::CancelFlag>,
    download_job: Option<crate::runtime::Job>,
    check_job: Option<crate::runtime::Job>,
    ink: FastInk,
    /// This frame is only a spinner tick. Those rectangles use an A2 update
    /// and do not wait.
    spinner_tick: bool,
    /// The download bar moved. Only that rectangle is an A2 update. The label
    /// above it stays a normal update, because A2 crushes the text.
    fast_refresh: Cell<bool>,
}

impl Ota {
    fn new(auth: device_flow::ResolvedAuth) -> Self {
        Self {
            list: List::new(),
            auth_panel: AuthPanel::new(),
            progress: Progress::new(),
            choice: ChoiceSheet::new(),
            confirm: ConfirmSheet::new(),
            back: CallbackQueue::new(),
            phase: Phase::Source,
            came_from: CameFrom::Source,
            auth,
            pending: None,
            confirm_pr: None,
            pager: Pager::new(10),
            rows: Vec::new(),
            list_generation: 0,
            loading: false,
            auth_message: String::new(),
            auth_code: String::new(),
            progress_label: String::new(),
            progress_percent: 0,
            progress_cancelable: true,
            download_canceling: false,
            cancelled: Arc::new(crate::http::CancelFlag::new()),
            download_in_progress: false,
            download_committed: false,
            pr_job: None,
            auth_job: None,
            auth_cancelled: Arc::new(crate::http::CancelFlag::new()),
            download_job: None,
            check_job: None,
            ink: FastInk::new(),
            spinner_tick: false,
            fast_refresh: Cell::new(false),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SlintScreen for Ota {
    type Window = OtaWindow;

    fn open(context: &mut AppContext) -> anyhow::Result<Session<OtaWindow>> {
        crate::view::slint::open(&context.device, |_family| {
            let ui = OtaWindow::new().context("failed to create slint ota window")?;
            apply_ink(&ui, context);
            Ok(ui)
        })
    }

    fn wire(&mut self, window: &OtaWindow) {
        connect!(window, self.back, on_back_pressed, () => ());
        self.list.wire(window);
        self.auth_panel.wire(window);
        self.progress.wire(window);
        self.choice.wire(window);
        self.confirm.wire(window);
    }

    fn start(&mut self) {
        self.show_source();
    }

    fn sync(&mut self, session: &Session<OtaWindow>, context: &AppContext) {
        let window = session.component();
        window.set_screen(match self.phase {
            Phase::Source => OtaScreen::Source,
            Phase::List => OtaScreen::List,
            Phase::Auth => OtaScreen::Auth,
            Phase::Progress => OtaScreen::Progress,
        });
        window.set_back_enabled(!self.download_in_progress && !self.download_committed);
        self.pager.set_per_page(
            u8::try_from(window.get_per_page())
                .ok()
                .filter(|count| *count > 0)
                .unwrap_or(10),
        );
        self.pager.clamp(self.rows.len());
        self.list
            .publish(window, self.pager, &self.rows, self.loading);
        self.auth_panel
            .publish(window, &self.auth_message, &self.auth_code);
        self.progress.publish(
            window,
            &self.progress_label,
            self.progress_percent,
            self.download_committed,
            self.progress_cancelable && !self.download_canceling,
        );
        self.choice.sync(window, context);
        self.confirm.sync(window, context);
        let _ = session.redraw();
        self.remember_bar(session, context);
    }

    async fn apply_callbacks(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        context: &mut AppContext,
    ) -> Flow {
        let mut redraw = false;
        let mut env = Env { hub, bus, context };
        for action in self.list.take() {
            let action = match self.list.apply(action, &mut env).await {
                list::Action::Line(index) => Action::Line(index),
                list::Action::Refresh => Action::Refresh,
                list::Action::Previous => Action::Previous,
                list::Action::Next => Action::Next,
            };
            if self
                .apply_action(action, env.hub, env.bus, env.context)
                .await
            {
                redraw = true;
            }
        }
        for action in self.auth_panel.take() {
            self.auth_panel.apply(action, &mut env).await;
            if self
                .apply_action(Action::AuthCancel, env.hub, env.bus, env.context)
                .await
            {
                redraw = true;
            }
        }
        for action in self.progress.take() {
            self.progress.apply(action, &mut env).await;
            if self
                .apply_action(Action::ProgressCancel, env.hub, env.bus, env.context)
                .await
            {
                redraw = true;
            }
        }
        for () in self.back.drain() {
            if self
                .apply_action(Action::Back, env.hub, env.bus, env.context)
                .await
            {
                redraw = true;
            }
        }
        for action in self.choice.take() {
            let outcome = self.choice.apply(action, &mut env).await;
            match outcome {
                choice_sheet::Outcome::Picked(source) => {
                    self.on_source(source, env.hub, env.context);
                    redraw = true;
                }
                choice_sheet::Outcome::Dismissed => {
                    env.bus.push_back(Event::Back);
                }
                choice_sheet::Outcome::Idle => {}
            }
        }
        for action in self.confirm.take() {
            let outcome = self.confirm.apply(action, &mut env).await;
            match outcome {
                confirm_sheet::Outcome::Accepted => {
                    if let Some(pr) = self.confirm_pr.take() {
                        self.start_pr(pr, env.hub, env.context);
                    }
                    redraw = true;
                }
                confirm_sheet::Outcome::Dismissed => {
                    self.confirm_pr = None;
                    redraw = true;
                }
                confirm_sheet::Outcome::Idle => {}
            }
        }
        if redraw { Flow::Redraw } else { Flow::Idle }
    }

    fn consume_back(&mut self) -> bool {
        if self.download_committed {
            return true;
        }
        if self.download_in_progress {
            self.begin_cancel();
            return true;
        }
        if self.phase == Phase::Source {
            return false;
        }
        {
            let mut overlays = OverlayStack::new(vec![&mut self.choice, &mut self.confirm]);
            if overlays.close_top() {
                self.confirm_pr = None;
                return true;
            }
        }
        self.check_job = None;
        self.show_source();
        true
    }

    async fn on_event(&mut self, evt: &Event, env: &mut Env<'_>, _rq: &mut RenderQueue) -> bool {
        match evt {
            Event::OtaDownloadProgress {
                label,
                percent,
                cancelable,
            } if self.phase == Phase::Progress => {
                self.on_progress(label, *percent, *cancelable);
                self.fast_refresh.set(true);
                true
            }
            Event::Ota(OtaEvent::PrListed { generation, rows }) => {
                self.on_pr_listed(*generation, rows.clone(), env.hub);
                true
            }
            Event::Ota(OtaEvent::PrStatus {
                generation,
                rows,
                failed,
            }) => {
                self.on_pr_status(*generation, rows.clone(), *failed, env.hub);
                true
            }
            Event::Ota(OtaEvent::PrListFailed { generation }) => {
                self.on_pr_list_failed(*generation, env.hub);
                true
            }
            Event::Ota(OtaEvent::DownloadFinished(_)) => {
                self.leave_progress();
                true
            }
            Event::Github(GithubEvent::DeviceAuthStarted {
                verification_uri,
                user_code,
            }) => {
                self.auth_message = fl!("device-auth-go-to-uri", uri = verification_uri.as_str());
                self.auth_code = user_code.clone();
                true
            }
            Event::Github(GithubEvent::DeviceAuthComplete(token)) => {
                self.on_auth_complete(token, env.hub, env.context).await;
                true
            }
            Event::Github(GithubEvent::TokenInvalid) => {
                self.on_token_invalid(env.hub, env.context);
                true
            }
            Event::Github(GithubEvent::DeviceAuthExpired) => {
                self.notify(env.hub, fl!("ota-auth-timed-out"));
                self.show_source();
                true
            }
            Event::Github(GithubEvent::DeviceAuthError(_)) => {
                self.notify(env.hub, fl!("ota-auth-error"));
                self.show_source();
                true
            }
            Event::Github(GithubEvent::StableReleaseCheck(outcome)) => {
                self.on_stable_release_check(outcome.clone(), env.hub, env.context);
                true
            }
            Event::StartStableReleaseDownload => {
                self.start_download(OtaDownloadKind::StableRelease, env.hub, env.context);
                true
            }
            _ => false,
        }
    }

    async fn take_device(
        &mut self,
        event: DeviceEvent,
        _window: Option<&OtaWindow>,
        _env: &mut Env<'_>,
        swallow: &mut Option<ButtonCode>,
    ) -> bool {
        match event {
            DeviceEvent::Button {
                code: code @ (ButtonCode::Backward | ButtonCode::Forward),
                status: ButtonStatus::Pressed | ButtonStatus::Repeated,
                ..
            } if self.can_page() => {
                let delta = if code == ButtonCode::Backward { -1 } else { 1 };
                *swallow = Some(code);
                self.step_page(delta);
                true
            }
            _ => false,
        }
    }

    fn panel_refresh(&self, _finger_down: bool) -> (UpdateMode, bool) {
        if self.spinner_tick {
            (UpdateMode::Fast, false)
        } else {
            (UpdateMode::Gui, true)
        }
    }

    fn queue_dirty(
        &self,
        id: Id,
        session: &Session<OtaWindow>,
        rq: &mut RenderQueue,
        _finger_down: bool,
    ) {
        let dirty = session.take_dirty();
        if self.spinner_tick {
            self.ink.queue_fast(id, dirty, UpdateMode::Fast, false, rq);
            return;
        }
        if self.fast_refresh.replace(false) {
            self.ink.queue_bar(id, dirty, rq);
            return;
        }
        self.ink.queue_with_clean(id, dirty, rq);
    }

    fn note_timer(&mut self, _repainted: bool) {
        self.spinner_tick = true;
    }

    fn clear_timer_note(&mut self) {
        self.spinner_tick = false;
    }

    fn stop_jobs(&self) {
        if self.download_in_progress {
            self.cancelled.request_cancel();
        }
        self.auth_cancelled.request_cancel();
        for job in [
            &self.pr_job,
            &self.auth_job,
            &self.download_job,
            &self.check_job,
        ]
        .into_iter()
        .flatten()
        {
            job.cancel();
        }
    }

    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        for job in [
            self.pr_job.take(),
            self.auth_job.take(),
            self.download_job.take(),
            self.check_job.take(),
        ]
        .into_iter()
        .flatten()
        {
            jobs.push(job);
        }
    }
}

impl OtaView {
    /// Opens the screen on the source menu.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(rq, context)))]
    pub async fn new(rect: Rectangle, rq: &mut RenderQueue, context: &mut AppContext) -> Self {
        let install_dir = context.device.install_dir();
        let auth = match crate::runtime::spawn_blocking(move || {
            device_flow::ResolvedAuth::load(&install_dir)
        })
        .await
        {
            Ok(Ok(auth)) => auth,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "Failed to load saved GitHub token");
                device_flow::ResolvedAuth::empty()
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to load saved GitHub token");
                device_flow::ResolvedAuth::empty()
            }
        };
        Self::mount(rect, rq, context, Ota::new(auth))
    }
}

impl Ota {
    #[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
    fn remember_bar(&self, session: &Session<OtaWindow>, context: &AppContext) {
        if self.phase != Phase::Progress {
            self.ink.set_bar(rect![0, 0, 0, 0]);
            return;
        }
        let window = session.component();
        publish_bar_geometry(window, || {
            let _ = session.redraw();
        });
        let scale = f32::from(context.device.dpi().max(1)) / crate::unit::BASE_DPI;
        let x = (window.get_bar_x() * scale).round() as i32;
        let y = (window.get_bar_y() * scale).round() as i32;
        let width = (window.get_bar_width() * scale).round() as i32;
        let height = (window.get_bar_height() * scale).round() as i32;
        self.ink.set_bar(rect![
            x,
            y,
            x.saturating_add(width),
            y.saturating_add(height)
        ]);
    }

    fn show_source(&mut self) {
        self.phase = Phase::Source;
        self.loading = false;
        self.confirm.close();
        self.confirm_pr = None;
        self.cancel_auth();
        self.pr_job = None;
        self.choice.open(fl!("ota-source-title"), source_rows());
    }

    fn notify(&self, hub: &Hub, message: String) {
        hub.send((Event::Notification(NotificationEvent::Show(message))).into())
            .ok();
    }

    async fn apply_action(
        &mut self,
        action: Action,
        hub: &Hub,
        bus: &mut Bus,
        context: &mut AppContext,
    ) -> bool {
        match action {
            Action::Back => {
                if !self.consume_back() {
                    bus.push_back(Event::Back);
                }
                true
            }
            Action::Line(index) => {
                self.on_line(index);
                true
            }
            Action::Refresh => {
                self.refresh(hub, context);
                true
            }
            Action::Previous => {
                self.step_page(-1);
                true
            }
            Action::Next => {
                self.step_page(1);
                true
            }
            Action::AuthCancel => {
                self.show_source();
                true
            }
            Action::ProgressCancel => {
                self.begin_cancel();
                true
            }
        }
    }

    fn on_source(&mut self, source: Source, hub: &Hub, context: &mut AppContext) {
        self.choice.close();
        match source {
            #[cfg(not(feature = "test"))]
            Source::Stable => {
                self.came_from = CameFrom::Source;
                self.enter_progress(OtaDownloadKind::StableRelease.progress_label(0));
                self.start_version_check(hub, context);
            }
            Source::Main => {
                if !self.ensure_token(Pending::Main, hub, context) {
                    return;
                }
                self.pending = Some(Pending::Main);
                self.came_from = CameFrom::Source;
                self.enter_progress(OtaDownloadKind::DefaultBranch.progress_label(0));
                self.start_download(OtaDownloadKind::DefaultBranch, hub, context);
            }
            Source::Pr => {
                if !self.ensure_token(Pending::PrList, hub, context) {
                    return;
                }
                self.pager.reset();
                self.show_list(hub, context);
            }
        }
    }

    fn ensure_token(&mut self, pending: Pending, hub: &Hub, context: &mut AppContext) -> bool {
        if self.auth.effective().is_some() {
            return true;
        }
        self.pending = Some(pending);
        self.start_auth(hub, context);
        false
    }

    fn start_auth(&mut self, hub: &Hub, context: &mut AppContext) {
        self.cancel_auth();
        self.phase = Phase::Auth;
        self.choice.close();
        self.auth_message = fl!("device-auth-connecting");
        self.auth_code.clear();
        self.auth_cancelled = Arc::new(crate::http::CancelFlag::new());
        self.auth_job = Some(spawn_device_flow_job(
            hub,
            Arc::clone(&self.auth_cancelled),
            context.wifi_session.clone(),
        ));
    }

    fn cancel_auth(&mut self) {
        self.auth_cancelled.request_cancel();
        self.auth_job = None;
    }

    fn show_list(&mut self, hub: &Hub, context: &mut AppContext) {
        self.phase = Phase::List;
        self.came_from = CameFrom::List;
        self.choice.close();
        if self.rows.is_empty() {
            self.load_list(hub, context);
        } else {
            self.loading = false;
        }
    }

    fn refresh(&mut self, hub: &Hub, context: &mut AppContext) {
        self.load_list(hub, context);
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, hub, context), fields(pager = ?self.pager))
    )]
    fn load_list(&mut self, hub: &Hub, context: &mut AppContext) {
        self.loading = self.rows.is_empty();
        self.list_generation = self.list_generation.wrapping_add(1);
        let generation = self.list_generation;
        let token = self.auth.effective();
        let wifi = context.wifi_session.clone();
        let tmp_dir = context.device.tmp_dir();
        let hub = hub.clone();
        self.pr_job = Some(crate::runtime::Job::spawn(move |cancel| async move {
            let mut listed = false;
            let outcome = crate::runtime::race_cancel(&cancel, async {
                let _wifi = wifi
                    .acquire("ota-pr-list")
                    .await
                    .map_err(|error| OtaError::Api(error.to_string()))?;
                let github =
                    GithubClient::new(token).map_err(|error| OtaError::Api(error.to_string()))?;
                let pulls = open_pulls(github.list_all_open_pulls().await?);
                let client = OtaClient::new(github, tmp_dir);
                listed = true;
                hub.send(
                    (Event::Ota(OtaEvent::PrListed {
                        generation,
                        rows: pending_pulls(&pulls),
                    }))
                    .into(),
                )
                .ok();
                let hub = hub.clone();
                client
                    .resolve_install_status(&pulls, move |checked, failed| {
                        hub.send(
                            (Event::Ota(OtaEvent::PrStatus {
                                generation,
                                rows: checked,
                                failed,
                            }))
                            .into(),
                        )
                        .ok();
                    })
                    .await
            })
            .await;
            if cancel.is_cancelled() {
                return;
            }
            match outcome {
                Ok(()) => {}
                Err(OtaError::Cancelled) => {}
                Err(error) if listed => {
                    tracing::error!(error = %error, "pull request install check failed");
                }
                Err(error) => {
                    tracing::error!(error = %error, "pull request list failed");
                    hub.send((Event::Ota(OtaEvent::PrListFailed { generation })).into())
                        .ok();
                }
            }
        }));
    }

    fn step_page(&mut self, delta: i32) {
        if !self.can_page() {
            return;
        }
        self.pager.step(delta, self.rows.len());
    }

    fn can_page(&self) -> bool {
        self.phase == Phase::List && !self.choice.is_open() && !self.confirm.is_open()
    }

    fn on_line(&mut self, index: i32) {
        let Some(index) = usize::try_from(index).ok() else {
            return;
        };
        let Some(row) = self.pager.slice(&self.rows).get(index) else {
            return;
        };
        if !row.selectable() {
            return;
        }
        let Some(artifact) = row.artifact else {
            return;
        };
        let resolved = ResolvedPr {
            number: row.number,
            artifact_id: artifact.id,
            size_in_bytes: artifact.size_in_bytes,
        };
        let title = row.title.clone();
        self.confirm_pr = Some(resolved);
        self.confirm.open(
            fl!(
                "ota-confirm-pr",
                pr_number = resolved.number,
                title = title.as_str()
            ),
            fl!("ota-install"),
        );
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, rows, _hub), fields(generation, rows = rows.len()))
    )]
    fn on_pr_listed(&mut self, generation: u64, rows: Vec<CheckedPull>, _hub: &Hub) {
        if generation != self.list_generation {
            return;
        }
        self.rows = rows;
        self.loading = false;
        self.pager.clamp(self.rows.len());
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, rows, hub), fields(generation, failed, rows = rows.len()))
    )]
    fn on_pr_status(&mut self, generation: u64, rows: Vec<CheckedPull>, failed: bool, hub: &Hub) {
        if generation != self.list_generation {
            return;
        }
        self.rows = rows;
        self.loading = false;
        self.pager.clamp(self.rows.len());
        if failed {
            self.notify(hub, fl!("ota-pr-status-failed"));
        }
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, hub), fields(generation))
    )]
    fn on_pr_list_failed(&mut self, generation: u64, hub: &Hub) {
        if generation != self.list_generation {
            return;
        }
        self.loading = false;
        self.notify(hub, fl!("ota-pr-list-failed"));
    }

    fn start_pr(&mut self, pr: ResolvedPr, hub: &Hub, context: &mut AppContext) {
        self.pending = Some(Pending::Pr(pr));
        self.came_from = CameFrom::List;
        self.enter_progress(
            OtaDownloadKind::Pr {
                number: pr.number,
                artifact_id: pr.artifact_id,
                size_in_bytes: pr.size_in_bytes,
            }
            .progress_label(0),
        );
        self.start_download(
            OtaDownloadKind::Pr {
                number: pr.number,
                artifact_id: pr.artifact_id,
                size_in_bytes: pr.size_in_bytes,
            },
            hub,
            context,
        );
    }

    fn enter_progress(&mut self, label: String) {
        self.phase = Phase::Progress;
        self.choice.close();
        self.confirm.close();
        self.progress_label = label;
        self.progress_percent = 0;
        self.progress_cancelable = true;
        self.download_canceling = false;
        self.download_committed = false;
    }

    fn begin_cancel(&mut self) {
        if !self.download_in_progress || self.download_committed || self.download_canceling {
            return;
        }
        self.download_canceling = true;
        self.cancelled.request_cancel();
        self.progress_label = fl!("ota-download-canceling");
        self.progress_cancelable = false;
    }

    fn on_progress(&mut self, label: &str, percent: u8, cancelable: bool) {
        if !cancelable {
            self.download_committed = true;
            self.progress_cancelable = false;
        }
        if !self.download_canceling {
            self.progress_label = label.to_owned();
        }
        self.progress_percent = percent;
    }

    fn leave_progress(&mut self) {
        self.download_in_progress = false;
        self.download_canceling = false;
        self.download_committed = false;
        self.check_job = None;
        match self.came_from {
            CameFrom::List => {
                self.phase = Phase::List;
                self.loading = false;
            }
            CameFrom::Source => self.show_source(),
        }
    }

    fn start_download(&mut self, kind: OtaDownloadKind, hub: &Hub, context: &mut AppContext) {
        let github_token = match kind {
            OtaDownloadKind::StableRelease => self.auth.effective(),
            _ => {
                let Some(token) = self.auth.effective() else {
                    tracing::error!("GitHub token is missing when starting download");
                    return;
                };
                Some(token)
            }
        };
        self.cancelled = Arc::new(crate::http::CancelFlag::new());
        self.download_in_progress = true;
        self.download_committed = false;
        self.download_canceling = false;
        self.download_job = Some(run_ota_download(OtaDownloadContext {
            kind,
            hub: hub.clone(),
            tmp_dir: context.device.tmp_dir(),
            install_dir: context.device.install_dir(),
            github_token,
            cancelled: Arc::clone(&self.cancelled),
            wifi_session: context.wifi_session.clone(),
            inhibitor: Arc::clone(&context.inhibitor),
        }));
    }

    #[cfg(not(feature = "test"))]
    fn start_version_check(&mut self, hub: &Hub, context: &mut AppContext) {
        let github_token = self.auth.effective();
        let hub = hub.clone();
        let wifi_session = context.wifi_session.clone();
        let tmp_dir = context.device.tmp_dir();
        self.check_job = Some(crate::runtime::Job::spawn(move |cancel| async move {
            let outcome = crate::runtime::race_cancel(
                &cancel,
                run_stable_release_check(github_token, tmp_dir, wifi_session),
            )
            .await;
            if cancel.is_cancelled() {
                return;
            }
            match outcome {
                Ok(outcome) => {
                    hub.send((Event::Github(GithubEvent::StableReleaseCheck(outcome))).into())
                        .ok();
                }
                Err(OtaError::Cancelled) => {}
                Err(error) => {
                    tracing::error!(error = %error, "Failed to check for updates");
                    hub.send(
                        (Event::Notification(NotificationEvent::Show(fl!(
                            "ota-check-updates-failed"
                        ))))
                        .into(),
                    )
                    .ok();
                    hub.send(
                        (Event::Ota(OtaEvent::DownloadFinished(DownloadOutcome::Failed))).into(),
                    )
                    .ok();
                }
            }
        }));
    }

    fn on_stable_release_check(
        &mut self,
        outcome: StableReleaseCheckOutcome,
        hub: &Hub,
        context: &mut AppContext,
    ) {
        self.check_job = None;
        match outcome {
            StableReleaseCheckOutcome::Older => {
                self.start_download(OtaDownloadKind::StableRelease, hub, context);
            }
            StableReleaseCheckOutcome::Equal => {
                self.notify(hub, fl!("ota-already-latest"));
                self.leave_progress();
            }
            StableReleaseCheckOutcome::Newer => {
                self.notify(hub, fl!("ota-version-newer"));
                self.leave_progress();
            }
            StableReleaseCheckOutcome::Incomparable => {
                self.notify(hub, fl!("ota-cannot-compare-versions"));
                self.leave_progress();
            }
        }
    }

    async fn on_auth_complete(
        &mut self,
        token: &SecretString,
        hub: &Hub,
        context: &mut AppContext,
    ) {
        if let Err(error) = device_flow::save_token(token, &context.device.install_dir()) {
            tracing::error!(error = %error, "Failed to save GitHub token");
        }
        self.auth.set_saved(token.clone());
        self.auth_job = None;
        self.resume(hub, context);
    }

    fn resume(&mut self, hub: &Hub, context: &mut AppContext) {
        match self.pending.clone() {
            Some(Pending::Main) => {
                self.came_from = CameFrom::Source;
                self.enter_progress(OtaDownloadKind::DefaultBranch.progress_label(0));
                self.start_download(OtaDownloadKind::DefaultBranch, hub, context);
            }
            Some(Pending::PrList) => {
                self.pager.reset();
                self.show_list(hub, context);
            }
            Some(Pending::Pr(pr)) => self.start_pr(pr, hub, context),
            None => self.show_source(),
        }
    }

    fn on_token_invalid(&mut self, hub: &Hub, context: &mut AppContext) {
        match self.auth.reject_effective() {
            Some(AuthOrigin::Environment) => {
                tracing::warn!("GH_TOKEN rejected — ignoring it for this session");
                if self.auth.effective().is_some() {
                    self.download_in_progress = false;
                    self.resume(hub, context);
                    return;
                }
            }
            Some(AuthOrigin::Saved) => {
                tracing::warn!("Saved GitHub token is invalid — clearing and re-authenticating");
                if let Err(error) = device_flow::delete_token(&context.device.install_dir()) {
                    tracing::error!(error = %error, "Failed to delete stale token");
                }
            }
            None => {
                tracing::warn!("GitHub token rejected with no token loaded");
            }
        }
        self.download_in_progress = false;
        self.download_committed = false;
        self.download_canceling = false;
        let pending = self.pending.clone().unwrap_or(Pending::PrList);
        self.start_auth(hub, context);
        self.pending = Some(pending);
    }
}

fn source_rows() -> Vec<Row<Source>> {
    let mut rows = Vec::new();
    #[cfg(not(feature = "test"))]
    rows.push(Row {
        label: fl!("ota-source-stable"),
        checked: false,
        value: Source::Stable,
    });
    rows.push(Row {
        label: fl!("ota-source-main"),
        checked: false,
        value: Source::Main,
    });
    rows.push(Row {
        label: fl!("ota-source-pr"),
        checked: false,
        value: Source::Pr,
    });
    rows
}

/// Keeps pull requests whose number fits the install check.
#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
fn open_pulls(listed: Vec<crate::github::ListedPull>) -> Vec<OpenPull> {
    listed
        .into_iter()
        .filter_map(|pull| {
            Some(OpenPull {
                number: u32::try_from(pull.number).ok()?,
                title: pull.title,
                draft: pull.draft,
                head_sha: pull.head_sha,
            })
        })
        .collect()
}

#[cfg(not(feature = "test"))]
async fn run_stable_release_check(
    github_token: Option<SecretString>,
    tmp_dir: std::path::PathBuf,
    wifi_session: Arc<crate::device::wifi::WifiSession>,
) -> Result<StableReleaseCheckOutcome, OtaError> {
    let _wifi = wifi_session
        .acquire("ota-version-check")
        .await
        .map_err(|error| OtaError::Api(error.to_string()))?;
    let github =
        GithubClient::new(github_token).map_err(|error| OtaError::Api(error.to_string()))?;
    let client = OtaClient::new(github, tmp_dir);
    let remote_version = client.fetch_latest_release_version().await?;
    let current_version = get_current_version();
    Ok(match current_version.compare(&remote_version).await? {
        VersionComparison::Equal => StableReleaseCheckOutcome::Equal,
        VersionComparison::Newer => StableReleaseCheckOutcome::Newer,
        VersionComparison::Older => StableReleaseCheckOutcome::Older,
        VersionComparison::Incomparable => StableReleaseCheckOutcome::Incomparable,
    })
}

/// Asks the progress page to copy the bar rectangle.
///
/// The page's binding does not run its change handler the first time it is
/// read. `redraw` evaluates that first read. The second write then sees the
/// laid-out bar.
#[cfg_attr(feature = "tracing", tracing::instrument(level = "trace", skip_all))]
fn publish_bar_geometry(window: &OtaWindow, redraw: impl FnOnce()) {
    let token = window.get_geom_token();
    window.set_geom_token(token.wrapping_add(1));
    redraw();
    window.set_geom_token(token.wrapping_add(2));
}

#[cfg(test)]
mod tests {
    use super::super::download::DownloadOutcome;
    use super::list::row_description;
    use super::*;
    use crate::ota::PrBuild;
    use crate::view::hub_channel;
    use crate::view::slint::SlintView;
    use crate::view::{NotificationEvent, View};

    fn bare() -> OtaView {
        SlintHost::blank(Ota::new(device_flow::ResolvedAuth::empty()))
    }

    #[test]
    fn a_spinner_tick_refreshes_the_dirty_sticks() {
        use crate::framebuffer::UpdateMode;

        let mut view = bare();
        assert_eq!(view.panel_refresh(false), (UpdateMode::Gui, true));
        view.spinner_tick = true;
        assert_eq!(view.panel_refresh(false), (UpdateMode::Fast, false));
    }

    #[test]
    fn the_progress_page_reports_the_bar_after_layout() {
        let font = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fonts/NotoSans-Regular.ttf"),
        )
        .expect("noto sans");
        let ui = crate::view::slint::open_with_font(300, 1264, 1680, &font, |_family| {
            let window = OtaWindow::new().context("ota window")?;
            window.set_screen(OtaScreen::Progress);
            window.set_progress_label("Downloading".into());
            window.set_progress(0.4);
            Ok(window)
        })
        .expect("ota session");
        assert!(ui.redraw());
        let window = ui.component();
        publish_bar_geometry(window, || {
            assert!(ui.redraw());
        });
        let x = window.get_bar_x();
        let y = window.get_bar_y();
        let width = window.get_bar_width();
        let height = window.get_bar_height();
        assert!(x > 20.0, "bar x {x}");
        assert!(width > 1000.0, "bar width {width}");
        assert!(height > 10.0, "bar height {height}");
        assert!(y > 100.0, "bar y {y}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn back_during_a_download_cancels_and_stays() {
        let mut view = bare();
        view.phase = Phase::Progress;
        view.download_in_progress = true;
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        let handled = view
            .handle_event(&Event::Back, &hub, &mut bus, &mut rq, &mut context)
            .await;

        assert!(handled);
        assert!(bus.is_empty());
        assert!(view.cancelled.is_cancelled());
        assert_eq!(view.progress_label, fl!("ota-download-canceling"));
        assert!(view.download_in_progress);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn back_after_commit_does_not_cancel() {
        let mut view = bare();
        view.phase = Phase::Progress;
        view.download_in_progress = true;
        view.download_committed = true;
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        let handled = view
            .handle_event(&Event::Back, &hub, &mut bus, &mut rq, &mut context)
            .await;

        assert!(handled);
        assert!(!view.cancelled.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_committed_progress_event_hides_cancel() {
        let mut view = bare();
        view.phase = Phase::Progress;
        view.download_in_progress = true;
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        view.handle_event(
            &Event::OtaDownloadProgress {
                label: "Installing…".into(),
                percent: 100,
                cancelable: false,
            },
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;

        assert!(view.download_committed);
        assert!(!view.progress_cancelable);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn take_background_jobs_collects_the_download_and_the_check() {
        let download = crate::runtime::Job::spawn(|token| async move {
            token.cancelled().await;
        });
        let check = crate::runtime::Job::spawn(|token| async move {
            token.cancelled().await;
        });
        let mut view = bare();
        view.download_job = Some(download);
        view.check_job = Some(check);

        let mut jobs = Vec::new();
        view.take_background_jobs(&mut jobs);
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_jobs_cancels_the_download_and_the_check() {
        let download = crate::runtime::Job::spawn(|token| async move {
            token.cancelled().await;
        });
        let check = crate::runtime::Job::spawn(|token| async move {
            token.cancelled().await;
        });
        let mut view = bare();
        view.download_in_progress = true;
        view.download_job = Some(download);
        view.check_job = Some(check);
        view.stop_jobs();
        assert!(view.cancelled.is_cancelled());
        drop(view);
    }

    #[test]
    fn a_draft_with_a_live_build_is_selectable_and_labelled() {
        let row = CheckedPull {
            number: 42,
            title: "Faster boots".into(),
            draft: true,
            build: PrBuild::Ready,
            artifact: Some(crate::ota::PrArtifact {
                id: 1,
                size_in_bytes: 10,
            }),
        };
        assert!(row.selectable());
        let description = row_description(&row);
        assert!(description.contains("42"), "{description}");
        assert!(
            description.contains(&fl!("ota-status-draft")),
            "{description}"
        );
        assert!(
            description.contains(&fl!("ota-status-ready")),
            "{description}"
        );
    }

    #[test]
    fn a_failed_row_is_not_selectable() {
        let row = CheckedPull {
            number: 3,
            title: "Broken".into(),
            draft: false,
            build: PrBuild::Failed,
            artifact: None,
        };
        assert!(!row.selectable());
    }

    #[test]
    fn a_pending_row_is_not_selectable() {
        let row = CheckedPull {
            number: 4,
            title: "Checking".into(),
            draft: false,
            build: PrBuild::Pending,
            artifact: None,
        };
        assert!(!row.selectable());
        let description = row_description(&row);
        assert!(description.contains('4'), "{description}");
        assert!(
            !description.contains(&fl!("ota-status-ready")),
            "{description}"
        );
    }

    #[test]
    fn next_stops_on_the_last_local_page() {
        let mut view = bare();
        view.phase = Phase::List;
        view.rows = (0..11).map(sample_row).collect();
        assert_eq!(view.pager.slice(&view.rows).len(), 10);
        let len = view.rows.len();
        view.pager.step(1, len);
        assert_eq!(view.pager.page(), 1);
        assert_eq!(view.pager.slice(&view.rows).len(), 1);
        assert_eq!(view.pager.slice(&view.rows)[0].number, 10);
        view.pager.step(1, len);
        assert_eq!(view.pager.page(), 1);
    }

    #[test]
    fn source_rows_match_the_build() {
        let rows = source_rows();
        #[cfg(feature = "test")]
        {
            assert_eq!(rows.len(), 2);
            assert!(matches!(rows[0].value, Source::Main));
            assert!(matches!(rows[1].value, Source::Pr));
        }
        #[cfg(not(feature = "test"))]
        {
            assert_eq!(rows.len(), 3);
            assert!(matches!(rows[0].value, Source::Stable));
            assert!(matches!(rows[1].value, Source::Main));
            assert!(matches!(rows[2].value, Source::Pr));
        }
    }

    #[test]
    fn open_pulls_drops_numbers_that_do_not_fit() {
        let pulls = open_pulls(vec![
            crate::github::ListedPull {
                number: 42,
                title: "Ok".into(),
                draft: true,
                head_sha: "abc".into(),
            },
            crate::github::ListedPull {
                number: u64::from(u32::MAX) + 1,
                title: "Too big".into(),
                draft: false,
                head_sha: "def".into(),
            },
        ]);
        assert_eq!(pulls.len(), 1);
        assert_eq!(pulls[0].number, 42);
        assert!(pulls[0].draft);
        assert_eq!(pulls[0].head_sha, "abc");
    }

    #[test]
    fn pending_rows_wait_for_the_install_check() {
        let rows = pending_pulls(&[OpenPull {
            number: 8,
            title: "Eight".into(),
            draft: false,
            head_sha: "h".into(),
        }]);
        assert_eq!(rows[0].number, 8);
        assert_eq!(rows[0].build, PrBuild::Pending);
        assert!(!rows[0].selectable());
    }

    #[test]
    fn a_checked_pull_keeps_its_install_answer() {
        let row = crate::ota::CheckedPull {
            number: 9,
            title: "Nine".into(),
            draft: false,
            build: PrBuild::Ready,
            artifact: Some(crate::ota::PrArtifact {
                id: 3,
                size_in_bytes: 8,
            }),
        };
        assert!(row.selectable());
        assert_eq!(row.artifact.unwrap().id, 3);
    }

    #[test]
    fn row_status_labels_cover_the_install_answers() {
        let cases = [
            (PrBuild::Building, fl!("ota-status-building")),
            (PrBuild::NoBuild, fl!("ota-status-no-build")),
            (PrBuild::Unknown, fl!("ota-status-unknown")),
            (PrBuild::Failed, fl!("ota-status-failed")),
        ];
        for (build, status) in cases {
            let description = row_description(&CheckedPull {
                number: 6,
                title: "Six".into(),
                draft: false,
                build,
                artifact: None,
            });
            assert!(description.contains(&status), "{build:?} -> {description}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_events_replace_rows_and_ignore_stale_jobs() {
        let mut view = bare();
        view.phase = Phase::List;
        view.list_generation = 3;
        view.loading = true;
        let (hub, mut rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        view.handle_event(
            &Event::Ota(OtaEvent::PrListed {
                generation: 2,
                rows: vec![sample_row(1)],
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert!(view.rows.is_empty());
        assert!(view.loading);

        view.handle_event(
            &Event::Ota(OtaEvent::PrListed {
                generation: 3,
                rows: vec![sample_row(1), sample_row(2)],
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.rows.len(), 2);
        assert!(!view.loading);
        assert_eq!(view.pager.page(), 0);

        view.handle_event(
            &Event::Ota(OtaEvent::PrStatus {
                generation: 1,
                rows: vec![ready_row(9)],
                failed: true,
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.rows[0].number, 1);

        view.handle_event(
            &Event::Ota(OtaEvent::PrStatus {
                generation: 3,
                rows: vec![ready_row(2)],
                failed: true,
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert!(view.rows[0].selectable());
        assert!(matches!(
            rx.try_recv().expect("status failed").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-pr-status-failed")
        ));

        view.handle_event(
            &Event::Ota(OtaEvent::PrListFailed { generation: 3 }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert!(matches!(
            rx.try_recv().expect("list failed").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-pr-list-failed")
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn progress_and_auth_events_return_to_the_right_screen() {
        let mut view = bare();
        view.phase = Phase::Progress;
        view.came_from = CameFrom::List;
        view.download_in_progress = true;
        let (hub, mut rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        view.handle_event(
            &Event::Ota(OtaEvent::DownloadFinished(DownloadOutcome::Cancelled)),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::List);
        assert!(!view.download_in_progress);

        view.phase = Phase::Progress;
        view.came_from = CameFrom::Source;
        view.handle_event(
            &Event::Github(GithubEvent::StableReleaseCheck(
                StableReleaseCheckOutcome::Equal,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Source);
        assert!(matches!(
            rx.try_recv().expect("already latest").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-already-latest")
        ));

        view.phase = Phase::Progress;
        view.came_from = CameFrom::Source;
        view.handle_event(
            &Event::Github(GithubEvent::StableReleaseCheck(
                StableReleaseCheckOutcome::Newer,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert!(matches!(
            rx.try_recv().expect("newer").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-version-newer")
        ));

        view.phase = Phase::Progress;
        view.handle_event(
            &Event::Github(GithubEvent::StableReleaseCheck(
                StableReleaseCheckOutcome::Incomparable,
            )),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert!(matches!(
            rx.try_recv().expect("incomparable").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-cannot-compare-versions")
        ));

        view.handle_event(
            &Event::Github(GithubEvent::DeviceAuthStarted {
                verification_uri: "https://example.test".into(),
                user_code: "ABCD-1234".into(),
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.auth_code, "ABCD-1234");
        assert_eq!(
            view.auth_message,
            fl!("device-auth-go-to-uri", uri = "https://example.test")
        );

        view.phase = Phase::Auth;
        view.handle_event(
            &Event::Github(GithubEvent::DeviceAuthExpired),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Source);
        assert!(matches!(
            rx.try_recv().expect("auth timed out").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-auth-timed-out")
        ));

        view.phase = Phase::Auth;
        view.handle_event(
            &Event::Github(GithubEvent::DeviceAuthError("nope".into())),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Source);
        assert!(matches!(
            rx.try_recv().expect("auth error").event,
            Event::Notification(NotificationEvent::Show(text))
                if text == fl!("ota-auth-error")
        ));

        view.phase = Phase::Auth;
        view.handle_event(
            &Event::Github(GithubEvent::DeviceAuthComplete(SecretString::from(
                "test_token",
            ))),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Source);
        assert!(view.auth.effective().is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn actions_page_confirm_and_leave_without_installing() {
        let mut view = bare();
        view.phase = Phase::List;
        view.rows = (0..11).map(sample_row).collect();
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        assert!(
            view.apply_action(Action::Next, &hub, &mut bus, &mut context)
                .await
        );
        assert_eq!(view.pager.page(), 1);
        assert!(
            view.apply_action(Action::Previous, &hub, &mut bus, &mut context)
                .await
        );
        assert_eq!(view.pager.page(), 0);

        view.apply_action(Action::Line(0), &hub, &mut bus, &mut context)
            .await;
        assert!(!view.confirm.is_open());

        view.rows = vec![ready_row(5)];
        view.apply_action(Action::Line(0), &hub, &mut bus, &mut context)
            .await;
        assert!(view.confirm.is_open());
        assert_eq!(view.confirm_pr.unwrap().number, 5);

        view.apply_action(Action::Line(-1), &hub, &mut bus, &mut context)
            .await;
        assert!(view.confirm.is_open());

        assert!(view.consume_back());
        assert!(!view.confirm.is_open());
        assert!(view.confirm_pr.is_none());

        view.phase = Phase::List;
        assert!(
            view.apply_action(Action::Back, &hub, &mut bus, &mut context)
                .await
        );
        assert_eq!(view.phase, Phase::Source);
        assert!(bus.is_empty());

        assert!(
            view.apply_action(Action::Back, &hub, &mut bus, &mut context)
                .await
        );
        assert!(matches!(bus.pop_front(), Some(Event::Back)));

        view.phase = Phase::Auth;
        view.apply_action(Action::AuthCancel, &hub, &mut bus, &mut context)
            .await;
        assert_eq!(view.phase, Phase::Source);

        view.phase = Phase::Progress;
        view.download_in_progress = true;
        view.apply_action(Action::ProgressCancel, &hub, &mut bus, &mut context)
            .await;
        assert!(view.cancelled.is_cancelled());
        assert_eq!(view.progress_label, fl!("ota-download-canceling"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn buttons_page_the_list_and_home_goes_back() {
        let mut view = bare();
        view.phase = Phase::List;
        view.rows = (0..11).map(sample_row).collect();
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;

        view.handle_event(
            &Event::Device(DeviceEvent::Button {
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
        assert_eq!(view.pager.page(), 1);
        assert_eq!(view.swallow_release, Some(ButtonCode::Forward));

        view.handle_event(
            &Event::Device(DeviceEvent::Button {
                time: 0.0,
                code: ButtonCode::Backward,
                status: ButtonStatus::Pressed,
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.pager.page(), 0);

        view.handle_event(
            &Event::Device(DeviceEvent::Button {
                time: 0.0,
                code: ButtonCode::Home,
                status: ButtonStatus::Pressed,
            }),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Source);
        assert!(view.choice.is_open());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn picking_pr_without_a_token_starts_auth() {
        let mut view = bare();
        let (hub, _rx) = hub_channel();
        let mut context = crate::context::test_helpers::create_test_context().await;
        view.on_source(Source::Pr, &hub, &mut context);
        assert_eq!(view.phase, Phase::Auth);
        assert!(matches!(view.pending, Some(Pending::PrList)));
        view.stop_jobs();
        let mut jobs = Vec::new();
        view.take_background_jobs(&mut jobs);
        drop(jobs);

        view.auth.set_saved(SecretString::from("test_token"));
        view.rows = vec![sample_row(1)];
        view.on_source(Source::Pr, &hub, &mut context);
        assert_eq!(view.phase, Phase::List);
        assert!(!view.loading);
        assert!(view.pr_job.is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_rejected_token_starts_auth_again() {
        let mut view = bare();
        let (hub, _rx) = hub_channel();
        let mut bus = Bus::new();
        let mut rq = RenderQueue::new();
        let mut context = crate::context::test_helpers::create_test_context().await;
        view.handle_event(
            &Event::Github(GithubEvent::TokenInvalid),
            &hub,
            &mut bus,
            &mut rq,
            &mut context,
        )
        .await;
        assert_eq!(view.phase, Phase::Auth);
        view.stop_jobs();
        let mut jobs = Vec::new();
        view.take_background_jobs(&mut jobs);
        drop(jobs);
    }

    fn sample_row(number: u32) -> CheckedPull {
        CheckedPull {
            number,
            title: format!("PR {number}"),
            draft: false,
            build: PrBuild::Pending,
            artifact: None,
        }
    }

    fn ready_row(number: u32) -> CheckedPull {
        CheckedPull {
            number,
            title: format!("PR {number}"),
            draft: false,
            build: PrBuild::Ready,
            artifact: Some(crate::ota::PrArtifact {
                id: 11,
                size_in_bytes: 22,
            }),
        }
    }
}
