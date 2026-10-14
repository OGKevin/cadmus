//! Generic Cadmus view that owns a Slint session.

use std::ops::{Deref, DerefMut};

use slint::{ComponentHandle, Global};

use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::framebuffer::UpdateMode;
use crate::geom::Rectangle;
use crate::input::{ButtonCode, ButtonStatus, DeviceEvent};
use crate::view::common::toggle_main_menu;
use crate::view::menu::Menu;
use crate::view::slint::generated::{InkMetrics, InkScreen, MaterialPalette};
use crate::view::slint::metrics::apply_ink;
use crate::view::slint::{Env, Flow, Session, SlintView, swallow_stale_press};
use crate::view::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId};

/// A generated window that uses the shared ink globals.
pub trait InkWindow: ComponentHandle + 'static {
    fn apply_ink(&self, context: &AppContext);
}

impl<W> InkWindow for W
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

/// Per-screen state owned by [`SlintHost`].
#[async_trait::async_trait(?Send)]
pub trait SlintScreen: Sized + 'static {
    type Window: InkWindow;

    fn open(context: &mut AppContext) -> anyhow::Result<Session<Self::Window>>;
    fn wire(&mut self, window: &Self::Window);
    fn start(&mut self) {}
    fn sync(&mut self, session: &Session<Self::Window>, context: &AppContext);

    async fn apply_callbacks(&mut self, hub: &Hub, bus: &mut Bus, context: &mut AppContext)
    -> Flow;

    fn consume_back(&mut self) -> bool {
        false
    }

    async fn on_event(&mut self, _evt: &Event, _env: &mut Env<'_>, _rq: &mut RenderQueue) -> bool {
        false
    }

    async fn take_device(
        &mut self,
        _event: DeviceEvent,
        _window: Option<&Self::Window>,
        _env: &mut Env<'_>,
        _swallow: &mut Option<ButtonCode>,
    ) -> bool {
        false
    }

    fn skip_preview(&self, _finger_down: bool, _finger_preview: bool) -> bool {
        false
    }

    fn panel_refresh(&self, _finger_down: bool) -> (UpdateMode, bool) {
        (UpdateMode::Gui, true)
    }

    fn queue_dirty(
        &self,
        id: Id,
        session: &Session<Self::Window>,
        rq: &mut RenderQueue,
        finger_down: bool,
    ) {
        let rects = session.take_dirty();
        if rects.is_empty() {
            return;
        }
        let (mode, wait) = self.panel_refresh(finger_down);
        for rect in rects {
            if wait {
                rq.add(RenderData::new(id, rect, mode));
            } else {
                rq.add(RenderData::no_wait(id, rect, mode));
            }
        }
    }

    fn note_timer(&mut self, _repainted: bool) {}
    fn clear_timer_note(&mut self) {}

    fn stop_jobs(&self) {}
    fn take_background_jobs(&mut self, jobs: &mut Vec<crate::runtime::Job>) {
        let _ = jobs;
    }

    fn log_resize(
        &self,
        _previous: Rectangle,
        _rect: Rectangle,
        _context: &AppContext,
        _session: Option<&Session<Self::Window>>,
        _children: usize,
    ) {
    }
}

/// Cadmus view that drives one [`SlintScreen`].
pub struct SlintHost<S: SlintScreen> {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    pub(crate) ui: Option<Session<S::Window>>,
    pub(crate) screen: S,
    pub(crate) swallow_release: Option<ButtonCode>,
}

impl<S: SlintScreen> Deref for SlintHost<S> {
    type Target = S;

    fn deref(&self) -> &S {
        &self.screen
    }
}

impl<S: SlintScreen> DerefMut for SlintHost<S> {
    fn deref_mut(&mut self) -> &mut S {
        &mut self.screen
    }
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

    pub(in crate::view) fn sync(&mut self, context: &AppContext) {
        let Some(session) = &self.ui else {
            return;
        };
        session.component().apply_ink(context);
        self.screen.sync(session, context);
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
            if !self.screen.consume_back() {
                bus.push_back(Event::Back);
            }
            self.sync(context);
            self.queue(rq);
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
            self.queue(rq);
            if let Some(session) = &self.ui {
                session.wake_for_timers(hub);
            }
            return;
        }
        if !swallow_stale_press(&mut self.swallow_release, &event) {
            self.drive(event, hub, bus, rq, context).await;
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
                    self.queue(rq);
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
                    self.queue(rq);
                    if let Some(session) = &self.ui {
                        session.wake_for_timers(hub);
                    }
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
        self.screen.log_resize(
            previous,
            rect,
            context,
            self.ui.as_ref(),
            self.children.len(),
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

#[async_trait::async_trait(?Send)]
impl<S: SlintScreen> SlintView for SlintHost<S> {
    type Component = S::Window;

    fn session(&self) -> Option<&Session<S::Window>> {
        self.ui.as_ref()
    }

    async fn apply_callbacks(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        context: &mut AppContext,
    ) -> Flow {
        self.screen.apply_callbacks(hub, bus, context).await
    }

    fn sync_window(&mut self, context: &AppContext) {
        self.sync(context);
    }

    fn skip_preview(&self, finger_down: bool, finger_preview: bool) -> bool {
        self.screen.skip_preview(finger_down, finger_preview)
    }

    fn panel_refresh(&self, finger_down: bool) -> (UpdateMode, bool) {
        self.screen.panel_refresh(finger_down)
    }

    fn queue_frame(&self, rq: &mut RenderQueue, finger_down: bool) {
        let Some(session) = &self.ui else {
            return;
        };
        self.screen.queue_dirty(self.id, session, rq, finger_down);
    }

    async fn on_timer(
        &mut self,
        hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut AppContext,
    ) {
        let repainted = self.session().is_some_and(Session::tick);
        let acted = self.apply_callbacks(hub, bus, context).await == Flow::Redraw;
        if acted {
            self.sync_window(context);
            self.queue_frame(rq, false);
        } else if repainted {
            self.screen.note_timer(true);
            self.queue_frame(rq, false);
            self.screen.clear_timer_note();
        }
        if let Some(session) = self.session() {
            session.wake_for_timers(hub);
        }
    }
}
