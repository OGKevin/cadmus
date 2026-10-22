//! Generic Cadmus view that owns a Slint session.

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::framebuffer::{Framebuffer as _, UpdateMode};
use crate::geom::Rectangle;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent, FingerStatus};
use crate::view::common::toggle_main_menu;
use crate::view::menu::Menu;
use crate::view::slint::generated::{InkMetrics, InkScreen, MaterialPalette};
use crate::view::slint::metrics::apply_ink;
use crate::view::slint::{Env, Flow, Session, swallow_stale_press};
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId};

/// A generated window that carries the shared ink globals.
///
/// The host writes the screen size, the hairline lengths, and the color-panel
/// flag into those globals before every sync.
pub trait InkGlobals: ComponentHandle + 'static {
    /// Writes the shared ink globals for this device.
    fn apply_ink(&self, context: &AppContext);
}

impl<W> InkGlobals for W
where
    W: ComponentHandle + 'static,
    for<'a> InkMetrics<'a>: Global<'a, W>,
    for<'a> InkScreen<'a>: Global<'a, W>,
    for<'a> MaterialPalette<'a>: Global<'a, W>,
{
    fn apply_ink(&self, context: &AppContext) {
        apply_ink(self, context);
    }
}

/// Why the window produced a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameCause {
    /// A device event reached the window.
    Input {
        /// A finger rests on the panel.
        finger_down: bool,
        /// A finger rests on the panel and may still move.
        finger_preview: bool,
        /// A callback changed the screen's state, so the window was re-synced.
        acted: bool,
    },
    /// A Slint timer fired.
    Timer {
        /// A callback changed the screen's state, so the window was re-synced.
        acted: bool,
    },
    /// The host changed the window without a device event.
    Sync,
}

/// One region of a frame and how it reaches the panel.
#[derive(Clone, Copy, Debug)]
pub struct RefreshRegion {
    /// The rectangle to draw.
    pub rect: Rectangle,
    /// The waveform to draw it with.
    pub mode: UpdateMode,
    /// Whether the panel waits for this region before the next frame.
    pub wait: bool,
}

/// How one frame reaches the panel.
///
/// The host fills the plan with the normal waveform. A screen that owns a
/// progress bar moves that region to the fast waveform, and a screen that
/// shows a press preview leaves the region out until the finger lifts.
#[derive(Clone, Debug, Default)]
pub struct RefreshPlan {
    /// The regions to draw.
    pub regions: Vec<RefreshRegion>,
}

impl RefreshPlan {
    /// Draws every rectangle with the normal waveform and waits for the frame.
    pub fn gui(&mut self, rects: Vec<Rectangle>) {
        self.push(rects, UpdateMode::Gui, true);
    }

    /// Draws every rectangle with the normal waveform without waiting.
    pub fn gui_no_wait(&mut self, rects: Vec<Rectangle>) {
        self.push(rects, UpdateMode::Gui, false);
    }

    /// Draws every rectangle with the fast waveform without waiting.
    pub fn fast(&mut self, rects: Vec<Rectangle>) {
        self.push(rects, UpdateMode::Fast, false);
    }

    /// Adds regions with an explicit waveform.
    pub fn push(&mut self, rects: Vec<Rectangle>, mode: UpdateMode, wait: bool) {
        self.regions.extend(
            rects
                .into_iter()
                .map(|rect| RefreshRegion { rect, mode, wait }),
        );
    }
}

/// The screen half of a Slint view.
///
/// A screen owns its window's state and the callbacks that window raises. The
/// host owns the session, the event loop, and the panel updates, so a new
/// screen is this trait plus a small `.slint` file and gets chrome, paging,
/// overlays, rotation, refresh, and the test harness for free.
///
/// The trait is `?Send` because a screen's state stays on the thread that
/// polls the view tree. Slint's platform, timers, and font collection are
/// thread-local.
#[async_trait::async_trait(?Send)]
pub trait SlintScreen: Sized + 'static {
    /// The generated window this screen draws.
    type Window: InkGlobals;

    /// Opens the window on the device framebuffer.
    fn open(context: &mut AppContext) -> anyhow::Result<Session<Self::Window>>;

    /// Connects the window's callbacks to the screen's queues.
    fn wire(&mut self, window: &Self::Window);

    /// Runs once after the window is wired and before the first sync.
    fn start(&mut self) {}

    /// Copies the screen's state into the window.
    ///
    /// The host redraws the window afterwards, so a screen never captures a
    /// frame itself.
    fn sync(&mut self, session: &Session<Self::Window>, context: &AppContext);

    /// Applies the callbacks the window queued during the last dispatch.
    ///
    /// The effect is the renderer instruction: [`Flow::Redraw`] means the
    /// window changed and the host must capture a frame.
    async fn apply_callbacks(&mut self, hub: &Hub, bus: &mut Bus, context: &mut AppContext)
    -> Flow;

    /// Closes the topmost sheet.
    ///
    /// Returns whether the screen took the Back, which keeps the view on this
    /// screen instead of leaving it.
    fn consume_back(&mut self) -> bool {
        false
    }

    /// Handles a view event that is not a device event.
    ///
    /// Returns whether the screen took it. The host syncs and queues a frame
    /// when it did.
    async fn on_event(&mut self, _evt: &Event, _env: &mut Env<'_>, _rq: &mut RenderQueue) -> bool {
        false
    }

    /// Handles a device event before it reaches the window.
    ///
    /// Returns whether the screen took it. The host syncs and queues a frame
    /// when it did. A screen that turns a press into its own action records
    /// the button in `swallow` so the release does not reach Slint.
    async fn take_device(
        &mut self,
        _event: DeviceEvent,
        _window: Option<&Self::Window>,
        _env: &mut Env<'_>,
        _swallow: &mut Option<ButtonCode>,
    ) -> bool {
        false
    }

    /// Plans how the dirty rectangles reach the panel.
    ///
    /// The default draws every rectangle with the normal waveform. A screen
    /// that owns a progress bar moves that region to the fast waveform, and
    /// a screen that shows a press preview leaves the region out until the
    /// finger lifts.
    fn refresh(&mut self, _cause: FrameCause, rects: Vec<Rectangle>, plan: &mut RefreshPlan) {
        plan.gui(rects);
    }

    /// Cancels the screen's jobs.
    fn stop_jobs(&self) {}

    /// Moves the screen's jobs to the caller, which joins them on shutdown.
    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        let _ = jobs;
    }
}

/// Cadmus view that drives one [`SlintScreen`].
pub struct SlintHost<S: SlintScreen> {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    pub(in crate::view) ui: Option<Session<S::Window>>,
    pub(in crate::view) screen: S,
    pub(in crate::view) swallow_release: Option<ButtonCode>,
}

impl<S: SlintScreen> SlintHost<S> {
    /// Opens `screen` on the device framebuffer and presents the first frame.
    pub(crate) fn mount(
        rect: Rectangle,
        rq: &mut RenderQueue,
        context: &mut AppContext,
        screen: S,
    ) -> Self {
        let ui = match S::open(context) {
            Ok(session) => Some(session),
            Err(error) => {
                tracing::error!(
                    error = %error,
                    screen = std::any::type_name::<S>(),
                    "slint screen failed to open"
                );
                None
            }
        };
        let mut host = Self {
            id: crate::view::ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            ui,
            screen,
            swallow_release: None,
        };
        if let Some(session) = &host.ui {
            host.screen.wire(session.component());
        }
        host.screen.start();
        host.sync(context);
        if let Some(session) = &host.ui {
            let framebuffer = context.device.framebuffer_mut();
            if let Err(error) = session.present(framebuffer) {
                tracing::error!(
                    error = %error,
                    screen = std::any::type_name::<S>(),
                    "slint frame failed"
                );
            }
        }
        rq.add(RenderData::new(host.id, host.rect, UpdateMode::Gui));
        host
    }

    #[cfg(test)]
    pub(crate) fn blank(screen: S) -> Self {
        Self {
            id: Id::default(),
            rect: rect![0, 0, 10, 10],
            children: Vec::new(),
            ui: None,
            screen,
            swallow_release: None,
        }
    }

    /// The screen this host drives.
    #[cfg(test)]
    pub(in crate::view) fn screen(&self) -> &S {
        &self.screen
    }

    /// The screen this host drives.
    #[cfg(test)]
    pub(in crate::view) fn screen_mut(&mut self) -> &mut S {
        &mut self.screen
    }

    /// The open session, if the screen has one.
    pub(in crate::view) fn session(&self) -> Option<&Session<S::Window>> {
        self.ui.as_ref()
    }

    /// Copies the screen's state into the window and captures a frame.
    pub(in crate::view) fn sync(&mut self, context: &AppContext) {
        let Some(session) = &self.ui else {
            return;
        };
        session.component().apply_ink(context);
        self.screen.sync(session, context);
        let _ = session.redraw();
    }

    /// Forwards one device event into the window, applies what it queued, and
    /// queues the dirty rectangles.
    async fn drive(
        &mut self,
        event: DeviceEvent,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let finger_down = matches!(
            event,
            DeviceEvent::Finger {
                status: FingerStatus::Down,
                ..
            }
        );
        let finger_preview = matches!(
            event,
            DeviceEvent::Finger {
                status: FingerStatus::Down | FingerStatus::Motion,
                ..
            }
        );
        let repainted = self
            .ui
            .as_ref()
            .is_some_and(|session| session.handle_device(event));
        let acted = self.screen.apply_callbacks(hub, bus, context).await == Flow::Redraw;
        if acted {
            self.sync(context);
            self.queue_frame(
                rq,
                FrameCause::Input {
                    finger_down,
                    finger_preview,
                    acted: true,
                },
            );
        } else if repainted {
            self.queue_frame(
                rq,
                FrameCause::Input {
                    finger_down,
                    finger_preview,
                    acted: false,
                },
            );
        }
        self.wake(hub);
    }

    /// Runs the due Slint timers, then the same path as [`Self::drive`].
    async fn on_timer(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let repainted = self.ui.as_ref().is_some_and(Session::tick);
        let acted = self.screen.apply_callbacks(hub, bus, context).await == Flow::Redraw;
        if acted {
            self.sync(context);
            self.queue_frame(rq, FrameCause::Timer { acted: true });
        } else if repainted {
            self.queue_frame(rq, FrameCause::Timer { acted: false });
        }
        self.wake(hub);
    }

    /// Asks the window to run its timers again when the next one is due.
    fn wake(&self, hub: &Hub) {
        if let Some(session) = &self.ui {
            session.wake_for_timers(hub);
        }
    }

    /// Queues the dirty rectangles the window produced.
    fn queue_frame(&mut self, rq: &mut RenderQueue, cause: FrameCause) {
        let Some(session) = &self.ui else {
            return;
        };
        let rects = session.take_dirty();
        if rects.is_empty() {
            return;
        }
        let mut plan = RefreshPlan::default();
        self.screen.refresh(cause, rects, &mut plan);
        for region in plan.regions {
            if region.wait {
                rq.add(RenderData::new(self.id, region.rect, region.mode));
            } else {
                rq.add(RenderData::no_wait(self.id, region.rect, region.mode));
            }
        }
    }

    async fn handle_device(
        &mut self,
        event: DeviceEvent,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        if let DeviceEvent::Button {
            code: ButtonCode::Home,
            status: ButtonStatus::Pressed,
            ..
        } = event
        {
            self.swallow_release = Some(ButtonCode::Home);
            if !self.screen.consume_back() {
                bus.push_back(Event::Back);
            }
            self.sync(context);
            self.queue_frame(rq, FrameCause::Sync);
            return;
        }

        let mut env = Env { hub, bus, context };
        let taken = {
            let window = self.ui.as_ref().map(Session::component);
            self.screen
                .take_device(event, window, &mut env, &mut self.swallow_release)
                .await
        };
        if taken {
            self.sync(context);
            self.queue_frame(rq, FrameCause::Sync);
            self.wake(hub);
            return;
        }
        if !swallow_stale_press(&mut self.swallow_release, &event) {
            self.drive(event, hub, bus, rq, context).await;
        }
    }

    /// Copies the captured frame into the device framebuffer.
    fn blit_frame(&self, context: &mut AppContext, rect: &Rectangle) {
        let Some(session) = self.session() else {
            return;
        };
        let (framebuffer, _, _) = context.framebuffer_and_fonts();
        if let Err(error) = session.blit_rect(framebuffer, rect) {
            tracing::error!(
                error = %error,
                %rect,
                screen = std::any::type_name::<S>(),
                "slint blit failed"
            );
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<S: SlintScreen> View for SlintHost<S> {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn stop_jobs(&self) {
        self.screen.stop_jobs();
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        self.screen.take_background_jobs(jobs);
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            level = "trace",
            skip(self, hub, bus, rq, context),
            fields(event = ?evt),
            ret(level = tracing::Level::TRACE)
        )
    )]
    async fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) -> bool {
        match evt {
            Event::Back => {
                if self.screen.consume_back() {
                    self.sync(context);
                    self.queue_frame(rq, FrameCause::Sync);
                    true
                } else {
                    false
                }
            }
            Event::ToggleNear(ViewId::MainMenu, rect) => {
                toggle_main_menu(self, *rect, None, rq, context);
                true
            }
            Event::Close(ViewId::MainMenu) => {
                toggle_main_menu(self, Rectangle::default(), Some(false), rq, context);
                true
            }
            Event::Device(device) => {
                self.handle_device(*device, hub, bus, rq, context).await;
                true
            }
            Event::SlintTimer => {
                self.on_timer(hub, bus, rq, context).await;
                true
            }
            other => {
                let mut env = Env { hub, bus, context };
                if self.screen.on_event(other, &mut env, rq).await {
                    self.sync(context);
                    self.queue_frame(rq, FrameCause::Sync);
                    self.wake(hub);
                    true
                } else {
                    false
                }
            }
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, context), fields(rect = ?rect)))]
    fn render(&self, context: &mut AppContext, rect: Rectangle) {
        self.blit_frame(context, &rect);
    }

    fn render_rect(&self, rect: &Rectangle) -> Rectangle {
        rect.intersection(self.rect()).unwrap_or(*rect)
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            level = "debug",
            skip(self, hub, rq, context),
            fields(rect = ?rect)
        )
    )]
    fn resize(
        &mut self,
        rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let previous = self.rect;
        self.rect = rect;
        let (width, height) = (rect.width(), rect.height());
        let size_changed = self
            .ui
            .as_ref()
            .is_some_and(|session| session.size() != (width, height));
        if size_changed {
            if let Some(session) = &mut self.ui {
                session.resize(width, height);
            }
            self.sync(context);
            if let Some(session) = &self.ui {
                session.take_dirty();
            }
            rq.add(RenderData::new(self.id, rect, UpdateMode::Full));
        }
        if previous.width() != width || previous.height() != height {
            self.children.retain(|child| !child.is::<Menu>());
            for child in &mut self.children {
                child.resize(rect, hub, rq, context);
            }
        }
        let fb_dims = context.device.framebuffer().dims();
        let session = self.ui.as_ref().map(|session| session.size());
        tracing::debug!(
            previous_rect = %previous,
            requested_rect = %rect,
            fb_width = fb_dims.0,
            fb_height = fb_dims.1,
            display_width = context.display.dims.0,
            display_height = context.display.dims.1,
            session_width = session.map(|size| size.0),
            session_height = session.map(|size| size.1),
            children = self.children.len(),
            "slint screen resize"
        );
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

    fn is_background(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::slint::generated::{SettingsCategory, SettingsWindow};
    use crate::view::slint::testing::open_screen;
    use crate::view::{Event, View};

    /// A screen that records what the host did.
    struct Stub {
        syncs: u32,
        causes: Vec<FrameCause>,
    }

    impl Stub {
        fn new() -> Self {
            Self {
                syncs: 0,
                causes: Vec::new(),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl SlintScreen for Stub {
        type Window = SettingsWindow;

        fn open(context: &mut AppContext) -> anyhow::Result<Session<SettingsWindow>> {
            crate::view::slint::open(&context.device, |_family| {
                SettingsWindow::new().map_err(|error| anyhow::anyhow!(error))
            })
        }

        fn wire(&mut self, _window: &SettingsWindow) {}

        fn sync(&mut self, session: &Session<SettingsWindow>, _context: &AppContext) {
            self.syncs += 1;
            let category = if self.syncs.is_multiple_of(2) {
                SettingsCategory::General
            } else {
                SettingsCategory::Power
            };
            session.component().set_category(category);
        }

        async fn apply_callbacks(
            &mut self,
            _hub: &Hub,
            _bus: &mut Bus,
            _context: &mut AppContext,
        ) -> Flow {
            Flow::Idle
        }

        fn refresh(&mut self, cause: FrameCause, rects: Vec<Rectangle>, plan: &mut RefreshPlan) {
            self.causes.push(cause);
            plan.gui(rects);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_host_redraws_after_a_sync() {
        let (context, mut host) = open_screen(Stub::new()).await;
        host.sync(&context);
        let session = host.session().expect("session");
        assert!(
            !session.take_dirty().is_empty(),
            "the host did not redraw the window after the sync"
        );
        assert_eq!(host.screen().syncs, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_resize_queues_a_full_refresh() {
        let (mut context, mut host) = open_screen(Stub::new()).await;
        let mut rq = RenderQueue::new();
        let (hub, _rx) = crate::view::hub_channel();
        let rotated = rect![0, 0, 800, 600];
        host.resize(rotated, &hub, &mut rq, &mut context);
        let modes: Vec<UpdateMode> = rq.iter().map(|((mode, _wait), _rects)| *mode).collect();
        assert!(
            modes.contains(&UpdateMode::Full),
            "a resize must refresh the whole panel, got {modes:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_resize_drops_the_main_menu() {
        let (mut context, mut host) = open_screen(Stub::new()).await;
        let mut rq = RenderQueue::new();
        crate::view::common::toggle_main_menu(
            &mut host,
            rect![0, 0, 600, 800],
            None,
            &mut rq,
            &mut context,
        );
        assert!(!host.children().is_empty(), "the menu did not open");
        let (hub, _rx) = crate::view::hub_channel();
        host.resize(rect![0, 0, 800, 600], &hub, &mut rq, &mut context);
        assert!(host.children().is_empty(), "the menu survived the resize");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_home_press_leaves_the_screen_and_is_swallowed() {
        let (mut context, mut host) = open_screen(Stub::new()).await;
        let mut rq = RenderQueue::new();
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        host.handle_event(
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
        assert_eq!(host.swallow_release, Some(ButtonCode::Home));
        assert!(
            bus.iter().any(|event| matches!(event, Event::Back)),
            "Home did not leave the screen"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_back_press_leaves_when_the_screen_does_not_take_it() {
        let (mut context, mut host) = open_screen(Stub::new()).await;
        let mut rq = RenderQueue::new();
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = Bus::new();
        let taken = host
            .handle_event(&Event::Back, &hub, &mut bus, &mut rq, &mut context)
            .await;
        assert!(!taken, "the screen took a Back it did not handle");
    }
}
