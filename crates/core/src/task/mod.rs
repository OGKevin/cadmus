//! Long-running background task infrastructure.
//!
//! This module provides a trait-based system for defining and managing
//! background tasks that run alongside the main application loop.
//!
//! # Architecture
//!
//! - [`BackgroundTask`] trait defines the interface for long-running tasks
//! - [`TaskManager`] spawns tasks on the process runtime and joins them on stop
//! - [`CancellationToken`] requests shutdown
//!
//! # Example
//!
//! ```no_run
//! use std::time::Duration;
//!
//! use cadmus_core::task::{sleep_unless_cancelled, BackgroundTask, TaskFuture, TaskId};
//! use cadmus_core::view::Hub;
//! use tokio_util::sync::CancellationToken;
//!
//! struct MyTask;
//!
//! impl BackgroundTask for MyTask {
//!     fn id(&self) -> TaskId {
//!         TaskId::Placeholder
//!     }
//!
//!     fn run<'a>(
//!         &'a mut self,
//!         _hub: &'a Hub,
//!         cancel: &'a CancellationToken,
//!     ) -> TaskFuture<'a> {
//!         Box::pin(async move {
//!             while !cancel.is_cancelled() {
//!                 if sleep_unless_cancelled(cancel, Duration::from_secs(60)).await {
//!                     break;
//!                 }
//!             }
//!         })
//!     }
//! }
//! ```

#[cfg(any(feature = "kobo", docsrs))]
mod auto_frontlight;
#[cfg(any(all(feature = "test", feature = "kobo"), docsrs))]
mod dbus_monitor;
mod dictionary_index;
#[cfg(any(feature = "test", docsrs))]
mod hello_world;
mod import;
mod thumbnail;
#[cfg(any(feature = "kobo", docsrs))]
mod time_sync;
#[cfg(any(feature = "kobo", docsrs))]
mod wifi_status_monitor;

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use thiserror::Error;

use crate::db::Database;
use crate::device::DeviceCapabilities as _;
#[cfg(feature = "kobo")]
use crate::device::DeviceHardware as _;
use crate::device::inhibitor::Inhibitor;
use crate::device::{AppContext, DeviceIdentity as _, DevicePaths as _};
#[cfg(feature = "kobo")]
use crate::fl;
use crate::input::DeviceEvent;
use crate::settings::Settings;
#[cfg(feature = "kobo")]
use crate::view::NotificationEvent;
use crate::view::{EntryId, Event};

/// Errors that can occur during task management operations.
#[derive(Error, Debug)]
pub enum TaskError {
    /// A task with the given ID is already running.
    #[error("task '{0}' is already running")]
    AlreadyRunning(TaskId),

    /// A task with the given ID is not running.
    #[error("task '{0}' is not running")]
    NotRunning(TaskId),
}

/// Unique identifier for a background task.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskId {
    /// A tmp placeholder until there is a Task always available.
    Placeholder,
    /// Library import task.
    Import,
    /// Thumbnail extraction background task.
    ThumbnailExtraction,
    /// Dictionary index background task.
    DictionaryIndex,
    /// The example task that prints periodically (test builds only).
    #[cfg(any(feature = "test", docsrs))]
    HelloWorld,
    /// D-Bus system bus monitor (test + kobo builds only).
    #[cfg(any(all(feature = "test", feature = "kobo"), docsrs))]
    DbusMonitor,
    /// WiFi status monitor using dhcpcd-dbus (kobo builds only).
    #[cfg(any(feature = "kobo", docsrs))]
    WifiStatusMonitor,
    /// Time synchronization via NTP (kobo builds only).
    #[cfg(any(feature = "kobo", docsrs))]
    TimeSync,
    /// Auto frontlight adjustment (kobo builds only).
    #[cfg(any(feature = "kobo", docsrs))]
    AutoFrontlight,
    /// Test-only task for unit tests.
    #[cfg(test)]
    TestTask,
    /// Second test-only task for unit tests.
    #[cfg(test)]
    TestTask2,
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskId::Placeholder => write!(f, "placeholder"),
            TaskId::Import => write!(f, "import"),
            TaskId::ThumbnailExtraction => write!(f, "thumbnail_extraction"),
            TaskId::DictionaryIndex => write!(f, "dictionary_index"),
            #[cfg(feature = "test")]
            TaskId::HelloWorld => write!(f, "hello_world"),
            #[cfg(all(feature = "test", feature = "kobo"))]
            TaskId::DbusMonitor => write!(f, "dbus_monitor"),
            #[cfg(feature = "kobo")]
            TaskId::WifiStatusMonitor => write!(f, "wifi_status_monitor"),
            #[cfg(feature = "kobo")]
            TaskId::TimeSync => write!(f, "time_sync"),
            #[cfg(feature = "kobo")]
            TaskId::AutoFrontlight => write!(f, "auto_frontlight"),
            #[cfg(test)]
            TaskId::TestTask => write!(f, "test_task"),
            #[cfg(test)]
            TaskId::TestTask2 => write!(f, "test_task_2"),
        }
    }
}

/// Future returned by [`BackgroundTask::run`].
///
/// Boxed so the trait stays object-safe. The task manager spawns it on the
/// process runtime, so the future must be [`Send`].
pub type TaskFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Sleeps until `duration` elapses or `cancel` is signalled.
///
/// Returns `true` when cancellation won. Periodic tasks use this so a stop
/// request interrupts the wait.
pub async fn sleep_unless_cancelled(cancel: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(duration) => false,
    }
}

/// A long-running background task.
///
/// Implement this trait to define tasks that run on the process runtime
/// alongside the main application loop. Tasks receive the event hub to
/// dispatch events and a [`CancellationToken`] to observe shutdown.
///
/// Polled tasks check [`CancellationToken::is_cancelled`] at the same
/// checkpoints they used to check for shutdown. Tasks that block on an
/// external wait race [`CancellationToken::cancelled`]. Time synchronisation
/// also checks between Wi-Fi lease, geolocation, NTP apply, and coordinate
/// publish so quit does not wait on the full sync or set the clock after
/// cancel.
pub trait BackgroundTask: Send {
    /// Returns the unique identifier for this task.
    fn id(&self) -> TaskId;

    /// Runs the task until it finishes or observes cancellation.
    ///
    /// Use `hub` to send events to the main loop and `cancel` to observe
    /// termination. The returned future runs on the process runtime.
    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a>;

    /// Called when the task is being stopped.
    ///
    /// Override this to perform cleanup. The default implementation does nothing.
    fn stop(&mut self) {}

    /// Returns a "finished" event to send after the task exits.
    ///
    /// The [`TaskManager`] calls this after [`run`](Self::run) and
    /// [`stop`](Self::stop) return, so the event can depend on the work's
    /// outcome. The default returns `None`.
    fn finished_event(&self) -> Option<Event> {
        None
    }
}

struct RunningTask {
    handle: tokio::task::JoinHandle<Option<Event>>,
    cancel: CancellationToken,
}

/// Manages the lifecycle of background tasks.
///
/// The task manager spawns tasks on the process runtime and provides
/// methods to stop individual tasks or all tasks at once.
pub struct TaskManager {
    tasks: HashMap<TaskId, RunningTask>,
    /// Library indices awaiting import while one is already running. The bool is the `force` flag.
    pending_import_indices: VecDeque<(Option<usize>, bool)>,
    /// Library indices awaiting thumbnail extraction while a run is in progress.
    pending_thumbnail_indices: VecDeque<Option<usize>>,
    /// Events from naturally finished tasks, waiting to be sent.
    buffered_events: Vec<Event>,
}

impl TaskManager {
    /// Creates a new empty task manager.
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
            pending_import_indices: VecDeque::new(),
            pending_thumbnail_indices: VecDeque::new(),
            buffered_events: Vec::new(),
        }
    }

    /// Starts a background task on the process runtime.
    ///
    /// The task receives a clone of `hub` for sending events and a
    /// [`CancellationToken`] for graceful termination.
    ///
    /// Returns an error if a task with the same ID is already running.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, task, hub), fields(task_id = tracing::field::Empty
    ), ret))]
    pub fn start(
        &mut self,
        task: Box<dyn BackgroundTask>,
        hub: crate::view::Hub,
    ) -> Result<TaskId, TaskError> {
        let id = task.id();

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("task_id", tracing::field::display(&id));

        if self.is_running(&id) {
            return Err(TaskError::AlreadyRunning(id));
        }

        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();

        let handle = crate::runtime::current_handle().spawn(async move {
            let mut task = task;
            tracing::info!("task started");
            task.run(&hub, &task_cancel).await;
            task.stop();
            tracing::info!("task stopped");
            task.finished_event()
        });

        self.tasks
            .insert(id.clone(), RunningTask { handle, cancel });

        tracing::info!("task registered");
        Ok(id)
    }

    /// Stops a running task by ID.
    ///
    /// Cancels the task and waits for it to finish.
    /// Returns an error if the task is not running.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(task_id = %id), ret))]
    pub fn stop(&mut self, id: &TaskId) -> Result<(), TaskError> {
        self.cleanup_finished();
        if let Some(task) = self.tasks.remove(id) {
            tracing::info!("cancelling task");
            task.cancel.cancel();
            if crate::runtime::block_on(task.handle).is_err() {
                tracing::error!("task panicked");
            }
            Ok(())
        } else {
            Err(TaskError::NotRunning(id.clone()))
        }
    }

    /// Stops all running tasks.
    ///
    /// Cancels every running task and waits for them to finish.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self), fields(task_count = tracing::field::Empty
    )))]
    /// Cancels every running task and waits for each join with a short deadline.
    ///
    /// Tasks that ignore cancel can still occupy a worker until the process
    /// runtime's own shutdown timeout; this bound keeps quit responsive.
    pub fn stop_all(&mut self) {
        let tasks: Vec<_> = self.tasks.drain().collect();

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("task_count", tasks.len());

        if !tasks.is_empty() {
            tracing::info!("stopping all tasks");
        }
        for (_, task) in &tasks {
            task.cancel.cancel();
        }
        const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
        for (id, task) in tasks {
            match crate::runtime::block_on(tokio::time::timeout(STOP_DEADLINE, task.handle)) {
                Ok(Ok(_)) => {}
                Ok(Err(_)) => tracing::error!(task_id = %id, "task panicked"),
                Err(_) => tracing::error!(task_id = %id, "task stop timed out"),
            }
        }
    }

    /// Removes entries for tasks whose futures have finished, buffering
    /// their completion events only if the task exited successfully.
    fn cleanup_finished(&mut self) {
        let finished: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.handle.is_finished())
            .map(|(id, _)| id.clone())
            .collect();

        for id in finished {
            if let Some(task) = self.tasks.remove(&id) {
                match crate::runtime::block_on(task.handle) {
                    Ok(Some(evt)) => self.buffered_events.push(evt),
                    Ok(None) => {}
                    Err(_) => tracing::error!(task_id = %id, "task panicked"),
                }
            }
        }
    }

    /// Sends any buffered completion events from naturally finished tasks.
    fn flush_buffered_events(&mut self, hub: &crate::view::Hub) {
        for evt in self.buffered_events.drain(..) {
            hub.send((evt).into()).ok();
        }
    }

    /// Observes an event without consuming it.
    ///
    /// Must be called for every event before passing it to the view tree.
    /// Always returns `false` — it never consumes events.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip(self, hub, context)))]
    pub fn handle_event(
        &mut self,
        evt: &Event,
        hub: &crate::view::Hub,
        context: &AppContext,
    ) -> bool {
        self.cleanup_finished();
        self.flush_buffered_events(hub);

        match evt {
            Event::ImportLibrary {
                library_index,
                force,
            } => {
                self.schedule_import(
                    *library_index,
                    *force,
                    hub,
                    &context.database,
                    &context.settings,
                    &context.device.install_dir(),
                    &context.inhibitor,
                );
            }
            Event::ImportFinished { library_index } => {
                self.drain_pending_imports(
                    hub,
                    &context.database,
                    &context.settings,
                    &context.device.install_dir(),
                    &context.inhibitor,
                );
                self.schedule_thumbnail_extraction(
                    *library_index,
                    hub,
                    &context.database,
                    &context.settings,
                    context.device.dpi(),
                    context.device.color_samples(),
                    &context.device.install_dir(),
                    &context.inhibitor,
                );
            }
            Event::ImportFailed { .. } => {
                self.drain_pending_imports(
                    hub,
                    &context.database,
                    &context.settings,
                    &context.device.install_dir(),
                    &context.inhibitor,
                );
            }
            Event::ThumbnailExtractionFinished { .. } => {
                self.drain_pending_thumbnails(
                    hub,
                    &context.database,
                    &context.settings,
                    context.device.dpi(),
                    context.device.color_samples(),
                    &context.device.install_dir(),
                    &context.inhibitor,
                );
            }
            Event::ReindexDictionaries => {
                self.schedule_dictionary_index(
                    hub,
                    &context.database,
                    context.device.data_dir(),
                    &context.inhibitor,
                );
            }
            Event::Device(DeviceEvent::NetUp) => {
                #[cfg(feature = "kobo")]
                {
                    if context.settings.auto_time {
                        self.schedule_time_sync(false, hub, context);
                    }
                }
            }
            #[cfg(feature = "kobo")]
            Event::AutoFrontlightConfigChanged => {
                self.sync_auto_frontlight(hub, &context.settings);
            }
            Event::Select(EntryId::SyncTime) => {
                #[cfg(feature = "kobo")]
                {
                    if !context.online && !context.settings.wifi.allows_on_demand() {
                        hub.send(
                            (Event::Notification(NotificationEvent::Show(fl!(
                                "notification-not-online"
                            ))))
                            .into(),
                        )
                        .ok();
                    } else {
                        self.schedule_time_sync(true, hub, context);
                    }
                }
            }
            _ => {}
        }
        false
    }

    /// Schedules an import task, queuing the index if one is already running.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn schedule_import(
        &mut self,
        library_index: Option<usize>,
        force: bool,
        hub: &crate::view::Hub,
        database: &Database,
        settings: &Settings,
        install_dir: &std::path::Path,
        inhibitor: &Arc<Inhibitor>,
    ) {
        if self.is_running(&TaskId::Import) {
            tracing::info!(library_index = ?library_index, force, "import already running, queueing");
            self.pending_import_indices
                .push_back((library_index, force));
            return;
        }

        self.flush_buffered_events(hub);

        let task = Box::new(import::ImportTask::new(
            database.clone(),
            settings.clone(),
            library_index,
            force,
            install_dir,
            inhibitor.clone(),
        ));

        if let Err(e) = self.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start import task");
        }
    }

    /// Starts the next pending import when the current one finishes.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn drain_pending_imports(
        &mut self,
        hub: &crate::view::Hub,
        database: &Database,
        settings: &Settings,
        install_dir: &std::path::Path,
        inhibitor: &Arc<Inhibitor>,
    ) {
        if self.is_running(&TaskId::Import) || self.pending_import_indices.is_empty() {
            return;
        }

        let Some((next, force)) = self.pending_import_indices.pop_front() else {
            return;
        };
        self.schedule_import(next, force, hub, database, settings, install_dir, inhibitor);
    }

    /// Schedules a dictionary index scan, stopping any running instance first.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn schedule_dictionary_index(
        &mut self,
        hub: &crate::view::Hub,
        database: &Database,
        data_path: std::path::PathBuf,
        inhibitor: &Arc<Inhibitor>,
    ) {
        if self.is_running(&TaskId::DictionaryIndex) {
            tracing::debug!("stopping running dictionary index task for restart");
            if let Err(e) = self.stop(&TaskId::DictionaryIndex) {
                tracing::warn!(error = %e, "failed to stop dictionary_index task for restart");
            }
        }

        self.flush_buffered_events(hub);

        let task = Box::new(dictionary_index::DictionaryIndexTask::new(
            database.clone(),
            data_path,
            inhibitor.clone(),
        ));

        if let Err(e) = self.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start dictionary_index task");
        }
    }

    #[cfg(feature = "kobo")]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn sync_auto_frontlight(&mut self, hub: &crate::view::Hub, settings: &Settings) {
        if self.is_running(&TaskId::AutoFrontlight) {
            tracing::debug!("stopping running auto_frontlight task for restart");
            if let Err(e) = self.stop(&TaskId::AutoFrontlight) {
                tracing::warn!(error = %e, "failed to stop auto_frontlight task for restart");
            }
        }

        if !settings.auto_frontlight {
            return;
        }

        self.flush_buffered_events(hub);

        let task = Box::new(auto_frontlight::AutoFrontlightTask);
        if let Err(e) = self.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start auto_frontlight task");
        }
    }

    #[cfg(feature = "kobo")]
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn schedule_time_sync(&mut self, manual: bool, hub: &crate::view::Hub, context: &AppContext) {
        if self.is_running(&TaskId::TimeSync) {
            tracing::warn!("Time sync task already running, not scheduling");

            return;
        }

        let Some(alarm_manager) = context.alarm_manager.as_ref() else {
            tracing::warn!("alarm manager unavailable, cannot sync time");
            return;
        };

        match context.device.time_manager() {
            Ok(time_manager) => {
                let task = Box::new(time_sync::TimeSyncTask::new(
                    time_manager.clone(),
                    context.settings.ntp_server.clone(),
                    manual,
                    context.wifi_session.clone(),
                    alarm_manager.clone(),
                ));
                if let Err(e) = self.start(task, hub.clone()) {
                    tracing::warn!(error = %e, "failed to start time sync task");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "time manager unavailable, cannot sync time");
            }
        }
    }

    /// Schedules a thumbnail extraction task, queuing the index if one is already running.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub fn schedule_thumbnail_extraction(
        &mut self,
        library_index: Option<usize>,
        hub: &crate::view::Hub,
        database: &Database,
        settings: &Settings,
        dpi: u16,
        color_samples: usize,
        install_dir: &std::path::Path,
        inhibitor: &Arc<Inhibitor>,
    ) {
        if self.is_running(&TaskId::ThumbnailExtraction) {
            tracing::info!(library_index = ?library_index, "thumbnail extraction already running, queueing");
            self.pending_thumbnail_indices.push_back(library_index);
            return;
        }

        self.flush_buffered_events(hub);

        let task = Box::new(thumbnail::ThumbnailExtractionTask::new(
            database.clone(),
            settings.clone(),
            library_index,
            dpi,
            color_samples,
            install_dir,
            inhibitor.clone(),
        ));

        if let Err(e) = self.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start thumbnail extraction task");
        }
    }

    /// Starts the next pending thumbnail extraction when the current one finishes.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn drain_pending_thumbnails(
        &mut self,
        hub: &crate::view::Hub,
        database: &Database,
        settings: &Settings,
        dpi: u16,
        color_samples: usize,
        install_dir: &std::path::Path,
        inhibitor: &Arc<Inhibitor>,
    ) {
        if self.is_running(&TaskId::ThumbnailExtraction)
            || self.pending_thumbnail_indices.is_empty()
        {
            return;
        }

        let Some(next) = self.pending_thumbnail_indices.pop_front() else {
            return;
        };

        self.schedule_thumbnail_extraction(
            next,
            hub,
            database,
            settings,
            dpi,
            color_samples,
            install_dir,
            inhibitor,
        );
    }

    /// Returns `true` if a task with the given ID is running.
    pub fn is_running(&mut self, id: &TaskId) -> bool {
        self.cleanup_finished();
        self.tasks.contains_key(id)
    }

    /// Returns the IDs of all running tasks.
    pub fn running_tasks(&mut self) -> Vec<TaskId> {
        self.cleanup_finished();
        self.tasks.keys().cloned().collect()
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}

/// Registers background tasks that run at startup.
///
/// Call this during startup to add background tasks.
/// Currently registers:
/// - [`wifi_status_monitor::WifiStatusMonitorTask`] - monitors WiFi status via dhcpcd-dbus (kobo only)
/// - [`hello_world::HelloWorldTask`] - prints "Hello world!" every minute (test only)
/// - [`dbus_monitor::DbusMonitorTask`] - monitors D-Bus signals (test + kobo only, when `settings.logging.enable_dbus_log` is true)
/// - [`import::ImportTask`] - runs an incremental import of all libraries on startup
/// - [`dictionary_index::DictionaryIndexTask`] - indexes `.index` dictionary files into SQLite
#[cfg_attr(feature = "tracing", tracing::instrument(skip(manager, hub, settings, database, data_path, install_dir, inhibitor), level = tracing::Level::TRACE))]
pub fn register_startup_tasks(
    manager: &mut TaskManager,
    hub: crate::view::Hub,
    settings: &Settings,
    database: &Database,
    data_path: std::path::PathBuf,
    install_dir: &std::path::Path,
    inhibitor: &Arc<Inhibitor>,
) {
    #[cfg(feature = "kobo")]
    {
        {
            let task = Box::new(wifi_status_monitor::WifiStatusMonitorTask);
            if let Err(e) = manager.start(task, hub.clone()) {
                tracing::warn!(error = %e, "failed to start wifi_status_monitor task");
            }
        }
        if settings.auto_frontlight {
            let task = Box::new(auto_frontlight::AutoFrontlightTask);
            if let Err(e) = manager.start(task, hub.clone()) {
                tracing::warn!(error = %e, "failed to start auto_frontlight task");
            }
        }
    }

    #[cfg(feature = "test")]
    {
        let task = Box::new(hello_world::HelloWorldTask);
        if let Err(e) = manager.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start hello_world task");
        }

        #[cfg(feature = "kobo")]
        if settings.logging.enable_dbus_log {
            let task = Box::new(dbus_monitor::DbusMonitorTask);
            if let Err(e) = manager.start(task, hub.clone()) {
                tracing::warn!(error = %e, "failed to start dbus_monitor task");
            }
        }
    }

    manager.schedule_import(
        None,
        false,
        &hub,
        database,
        settings,
        install_dir,
        inhibitor,
    );

    let task = Box::new(dictionary_index::DictionaryIndexTask::new(
        database.clone(),
        data_path,
        inhibitor.clone(),
    ));
    if let Err(e) = manager.start(task, hub.clone()) {
        tracing::warn!(error = %e, "failed to start dictionary_index task");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_helpers::create_test_context;
    use crate::view::HubReceiverExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn running_until_cancelled(result: Option<Event>) -> RunningTask {
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let handle = crate::runtime::current_handle().spawn(async move {
            child.cancelled().await;
            result
        });
        RunningTask { handle, cancel }
    }

    #[tokio::test]
    async fn idle_wait_stops_without_sleeping_the_full_interval() {
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let started = Instant::now();
        let handle =
            tokio::spawn(
                async move { sleep_unless_cancelled(&child, Duration::from_secs(60)).await },
            );
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
        assert!(handle.await.expect("sleep task panicked"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn polled_wifi_lease_drops_at_the_next_checkpoint() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let wifi = std::sync::Arc::new(crate::device::wifi::NoopWifiManager::default());
        let session = crate::device::wifi::WifiSession::new(wifi, crate::settings::WifiMode::Auto);
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let continued = std::sync::Arc::new(AtomicBool::new(false));
        let entered_task = std::sync::Arc::clone(&entered);
        let release_task = std::sync::Arc::clone(&release);
        let continued_task = std::sync::Arc::clone(&continued);
        let session_task = std::sync::Arc::clone(&session);

        let handle = tokio::spawn(async move {
            let _lease = session_task.acquire("time-sync").await.expect("wifi lease");
            entered_task.notify_one();
            release_task.notified().await;
            if child.is_cancelled() {
                return;
            }
            continued_task.store(true, Ordering::SeqCst);
        });

        entered.notified().await;
        assert!(session.has_holders());
        cancel.cancel();
        assert!(
            session.has_holders(),
            "a polled holder keeps the lease until the next checkpoint"
        );
        release.notify_one();
        handle.await.expect("lease task panicked");
        assert!(!continued.load(Ordering::SeqCst));
        assert!(!session.has_holders());
    }

    fn wait_until_not_running(manager: &mut TaskManager, id: &TaskId) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if !manager.is_running(id) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("task '{id}' did not finish within timeout");
    }

    struct InstantTask;

    impl BackgroundTask for InstantTask {
        fn id(&self) -> TaskId {
            TaskId::TestTask2
        }

        fn run<'a>(
            &'a mut self,
            _hub: &'a crate::view::Hub,
            _cancel: &'a CancellationToken,
        ) -> TaskFuture<'a> {
            Box::pin(async {})
        }
    }

    struct WaitingTask;

    impl BackgroundTask for WaitingTask {
        fn id(&self) -> TaskId {
            TaskId::TestTask
        }

        fn run<'a>(
            &'a mut self,
            _hub: &'a crate::view::Hub,
            cancel: &'a CancellationToken,
        ) -> TaskFuture<'a> {
            Box::pin(async move {
                sleep_unless_cancelled(cancel, Duration::from_secs(60)).await;
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_and_stop() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        let id = manager.start(Box::new(WaitingTask), hub).unwrap();
        assert!(manager.is_running(&id));

        manager.stop(&id).unwrap();
        assert!(!manager.is_running(&id));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn duplicate_start_returns_error() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        manager.start(Box::new(WaitingTask), hub.clone()).unwrap();
        let err = manager.start(Box::new(WaitingTask), hub).unwrap_err();

        assert!(matches!(err, TaskError::AlreadyRunning(TaskId::TestTask)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn finished_task_is_cleaned_up() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        let id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &id);
        assert!(!manager.is_running(&id));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_finished_task_returns_not_running() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        let id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &id);
        let err = manager.stop(&id).unwrap_err();

        assert!(matches!(err, TaskError::NotRunning(TaskId::TestTask2)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn running_tasks_excludes_finished() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        manager.start(Box::new(WaitingTask), hub.clone()).unwrap();
        let instant_id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &instant_id);
        let running = manager.running_tasks();

        assert_eq!(running.len(), 1);
        assert_eq!(running[0], TaskId::TestTask);

        manager.stop_all();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_all_stops_everything() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        manager.start(Box::new(WaitingTask), hub).unwrap();
        manager.stop_all();

        assert!(!manager.is_running(&TaskId::TestTask));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_thumbnail_extraction_task_lifecycle() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();
        let mut database = Database::new(":memory:").await.unwrap();
        database.init_for_test(0).await.unwrap();
        let settings = Settings::default();
        let context = create_test_context();

        manager.schedule_thumbnail_extraction(
            None,
            &hub,
            &database,
            &settings,
            300,
            1,
            Path::new(""),
            &context.inhibitor,
        );

        // Task exits quickly on an unseeded database, so wait for
        // completion rather than asserting the transient running state.
        wait_until_not_running(&mut manager, &TaskId::ThumbnailExtraction);
        assert!(!manager.is_running(&TaskId::ThumbnailExtraction));

        let err = manager.stop(&TaskId::ThumbnailExtraction).unwrap_err();
        assert!(matches!(
            err,
            TaskError::NotRunning(TaskId::ThumbnailExtraction)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn thumbnail_extraction_queues_when_running() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();

        manager
            .tasks
            .insert(TaskId::ThumbnailExtraction, running_until_cancelled(None));

        let mut database = Database::new(":memory:").await.unwrap();
        database.init_for_test(0).await.unwrap();
        let settings = Settings::default();
        let context = create_test_context();

        manager.schedule_thumbnail_extraction(
            Some(0),
            &hub,
            &database,
            &settings,
            300,
            1,
            Path::new(""),
            &context.inhibitor,
        );
        manager.schedule_thumbnail_extraction(
            Some(1),
            &hub,
            &database,
            &settings,
            300,
            1,
            Path::new(""),
            &context.inhibitor,
        );

        assert_eq!(manager.pending_thumbnail_indices.len(), 2);

        manager.stop(&TaskId::ThumbnailExtraction).unwrap();

        manager.drain_pending_thumbnails(
            &hub,
            &database,
            &settings,
            300,
            1,
            Path::new(""),
            &context.inhibitor,
        );
        assert_eq!(manager.pending_thumbnail_indices.len(), 1);

        wait_until_not_running(&mut manager, &TaskId::ThumbnailExtraction);

        manager.drain_pending_thumbnails(
            &hub,
            &database,
            &settings,
            300,
            1,
            Path::new(""),
            &context.inhibitor,
        );
        assert!(manager.pending_thumbnail_indices.is_empty());

        wait_until_not_running(&mut manager, &TaskId::ThumbnailExtraction);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn import_queue_preserves_force_flag() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();
        let context = create_test_context();

        manager
            .tasks
            .insert(TaskId::Import, running_until_cancelled(None));

        manager.handle_event(
            &Event::ImportLibrary {
                library_index: Some(0),
                force: true,
            },
            &hub,
            &context,
        );

        assert_eq!(
            manager.pending_import_indices.front(),
            Some(&(Some(0), true))
        );

        manager.stop(&TaskId::Import).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn import_queue_preserves_force_false_flag() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = crate::view::hub_channel();
        let context = create_test_context();

        manager
            .tasks
            .insert(TaskId::Import, running_until_cancelled(None));

        manager.handle_event(
            &Event::ImportLibrary {
                library_index: None,
                force: false,
            },
            &hub,
            &context,
        );

        assert_eq!(manager.pending_import_indices.front(), Some(&(None, false)));

        manager.stop(&TaskId::Import).unwrap();
    }

    fn library_settings(path: &Path) -> crate::settings::LibrarySettings {
        crate::settings::LibrarySettings {
            name: "test".to_string(),
            path: path.to_path_buf(),
            ..Default::default()
        }
    }

    fn import_context(dir: &Path) -> AppContext {
        let mut context = create_test_context();
        context.settings.libraries = vec![library_settings(dir)];
        context
    }

    fn pump(manager: &mut TaskManager, hub: &crate::view::Hub, context: &AppContext) {
        manager.handle_event(&Event::StartStableReleaseDownload, hub, context);
    }

    fn thumbnail_was_scheduled(
        manager: &mut TaskManager,
        hub: &crate::view::Hub,
        rx: &mut crate::view::HubReceiver,
        context: &AppContext,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if manager.is_running(&TaskId::ThumbnailExtraction) {
                return true;
            }
            pump(manager, hub, context);
            if rx
                .try_iter()
                .any(|message| matches!(message.event, Event::ThumbnailExtractionFinished { .. }))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn interrupted_import_emits_no_completion_and_does_not_schedule_thumbnails() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write");
        let context = import_context(dir.path());
        let (hub, mut rx) = crate::view::hub_channel();
        let mut task = import::ImportTask::new(
            context.database.clone(),
            context.settings.clone(),
            Some(0),
            false,
            context.device.install_dir(),
            context.inhibitor.clone(),
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        crate::runtime::block_on(task.run(&hub, &cancel));

        assert!(
            task.finished_event().is_none(),
            "interrupted import must not announce completion"
        );
        assert!(
            !rx.try_iter()
                .any(|message| matches!(message.event, Event::ImportFinished { .. })),
            "interrupted import must not emit ImportFinished"
        );

        let mut manager = TaskManager::new();
        if let Some(evt) = task.finished_event() {
            manager.handle_event(&evt, &hub, &context);
        }

        assert!(!manager.is_running(&TaskId::ThumbnailExtraction));
        assert!(manager.pending_thumbnail_indices.is_empty());
        assert!(
            !rx.try_iter()
                .any(|message| matches!(message.event, Event::ThumbnailExtractionFinished { .. })),
            "thumbnail extraction must not be scheduled after an interrupted import"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_import_emits_no_completion_and_does_not_schedule_thumbnails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good");
        std::fs::create_dir(&good).expect("mkdir");
        std::fs::write(good.join("book.epub"), b"epub content").expect("write");
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, b"x").expect("write");
        let mut context = create_test_context();
        context.settings.libraries = vec![
            library_settings(&good),
            library_settings(&blocker.join("library")),
        ];
        let (hub, mut rx) = crate::view::hub_channel();
        let mut task = import::ImportTask::new(
            context.database.clone(),
            context.settings.clone(),
            None,
            false,
            context.device.install_dir(),
            context.inhibitor.clone(),
        );
        let cancel = CancellationToken::new();
        crate::runtime::block_on(task.run(&hub, &cancel));

        assert!(
            matches!(
                task.finished_event(),
                Some(Event::ImportFailed {
                    library_index: None
                })
            ),
            "failed import must emit a terminal ImportFailed event"
        );
        assert!(
            !rx.try_iter()
                .any(|message| matches!(message.event, Event::ImportFinished { .. })),
            "failed import must not emit ImportFinished"
        );

        let mut manager = TaskManager::new();
        if let Some(evt) = task.finished_event() {
            manager.handle_event(&evt, &hub, &context);
        }

        assert!(!manager.is_running(&TaskId::ThumbnailExtraction));
        assert!(manager.pending_thumbnail_indices.is_empty());
        assert!(
            !rx.try_iter()
                .any(|message| matches!(message.event, Event::ThumbnailExtractionFinished { .. })),
            "thumbnail extraction must not be scheduled after a failed import"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_import_advances_queued_import_without_scheduling_thumbnails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good");
        std::fs::create_dir(&good).expect("mkdir");
        std::fs::write(good.join("book.epub"), b"epub content").expect("write");
        let mut context = create_test_context();
        context.settings.libraries = vec![library_settings(&good)];
        let (hub, mut rx) = crate::view::hub_channel();
        let mut manager = TaskManager::new();

        manager.tasks.insert(
            TaskId::Import,
            running_until_cancelled(Some(Event::ImportFailed {
                library_index: Some(0),
            })),
        );

        manager.handle_event(
            &Event::ImportLibrary {
                library_index: Some(0),
                force: false,
            },
            &hub,
            &context,
        );
        assert_eq!(
            manager.pending_import_indices.front(),
            Some(&(Some(0), false))
        );

        manager.stop(&TaskId::Import).unwrap();
        manager.handle_event(
            &Event::ImportFailed {
                library_index: Some(0),
            },
            &hub,
            &context,
        );

        assert!(manager.pending_import_indices.is_empty());
        assert!(!manager.is_running(&TaskId::ThumbnailExtraction));
        assert!(manager.pending_thumbnail_indices.is_empty());
        assert!(
            !rx.try_iter()
                .any(|message| matches!(message.event, Event::ThumbnailExtractionFinished { .. })),
            "failed import must not schedule thumbnail extraction"
        );

        wait_until_not_running(&mut manager, &TaskId::Import);
        pump(&mut manager, &hub, &context);

        let events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::ImportFinished {
                    library_index: Some(0)
                }
            )),
            "queued import must run after the preceding import fails"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::ImportFailed { .. })),
            "the queued import must not fail"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn completed_import_emits_completion_and_schedules_thumbnails() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("book.epub"), b"epub content").expect("write");
        let context = import_context(dir.path());
        let (hub, mut rx) = crate::view::hub_channel();
        let mut manager = TaskManager::new();
        let task = import::ImportTask::new(
            context.database.clone(),
            context.settings.clone(),
            Some(0),
            false,
            context.device.install_dir(),
            context.inhibitor.clone(),
        );
        manager.start(Box::new(task), hub.clone()).unwrap();
        wait_until_not_running(&mut manager, &TaskId::Import);
        pump(&mut manager, &hub, &context);

        let events: Vec<Event> = rx.try_iter().map(|message| message.event).collect();
        let finished = events
            .into_iter()
            .find(|event| {
                matches!(
                    event,
                    Event::ImportFinished {
                        library_index: Some(0)
                    }
                )
            })
            .expect("completed import must emit ImportFinished");

        manager.handle_event(&finished, &hub, &context);
        assert!(
            thumbnail_was_scheduled(&mut manager, &hub, &mut rx, &context),
            "completed import must schedule thumbnail extraction"
        );
    }
}
