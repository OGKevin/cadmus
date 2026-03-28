//! Long-running background task infrastructure.
//!
//! This module provides a trait-based system for defining and managing
//! background tasks that run alongside the main application loop.
//!
//! # Architecture
//!
//! - [`BackgroundTask`] trait defines the interface for long-running tasks
//! - [`TaskManager`] spawns and manages task lifecycles
//! - [`ShutdownSignal`] provides graceful shutdown coordination
//!
//! # Example
//!
//! ```ignore
//! use cadmus_core::task::{BackgroundTask, TaskId, ShutdownSignal};
//! use std::sync::mpsc::Sender;
//! use cadmus_core::view::Event;
//!
//! struct MyTask;
//!
//! impl BackgroundTask for MyTask {
//!     fn id(&self) -> TaskId {
//!         TaskId::MyTask
//!     }
//!
//!     fn run(&mut self, hub: &Sender<Event>, shutdown: &ShutdownSignal) {
//!         while !shutdown.should_stop() {
//!             // Do work...
//!             if shutdown.wait(Duration::from_secs(60)) {
//!                 break;
//!             }
//!         }
//!     }
//! }
//! ```

#[cfg(any(all(feature = "test", feature = "kobo"), doc))]
mod dbus_monitor;
#[cfg(any(feature = "test", doc))]
mod hello_world;
#[cfg(any(feature = "kobo", doc))]
mod wifi_status_monitor;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use thiserror::Error;

use crate::settings::Settings;
use crate::view::Event;

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
    /// The example task that prints periodically (test builds only).
    #[cfg(any(feature = "test", doc))]
    HelloWorld,
    /// D-Bus system bus monitor (test + kobo builds only).
    #[cfg(any(all(feature = "test", feature = "kobo"), doc))]
    DbusMonitor,
    /// WiFi status monitor using dhcpcd-dbus (kobo builds only).
    #[cfg(any(feature = "kobo", doc))]
    WifiStatusMonitor,
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
            #[cfg(feature = "test")]
            TaskId::HelloWorld => write!(f, "hello_world"),
            #[cfg(all(feature = "test", feature = "kobo"))]
            TaskId::DbusMonitor => write!(f, "dbus_monitor"),
            #[cfg(feature = "kobo")]
            TaskId::WifiStatusMonitor => write!(f, "wifi_status_monitor"),
            #[cfg(test)]
            TaskId::TestTask => write!(f, "test_task"),
            #[cfg(test)]
            TaskId::TestTask2 => write!(f, "test_task_2"),
        }
    }
}

/// Signal for coordinating graceful shutdown of background tasks.
///
/// Tasks should periodically check [`should_stop`](Self::should_stop) or use
/// [`wait`](Self::wait) to interrupt sleep when shutdown is requested.
pub struct ShutdownSignal {
    receiver: Receiver<()>,
    stopped: AtomicBool,
}

impl ShutdownSignal {
    fn new(receiver: Receiver<()>) -> Self {
        Self {
            receiver,
            stopped: AtomicBool::new(false),
        }
    }

    /// Returns `true` if shutdown has been requested.
    ///
    /// Once `true` is returned, all subsequent calls also return `true`
    /// (the shutdown state is latched). This is non-blocking and suitable
    /// for polling in tight loops.
    pub fn should_stop(&self) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return true;
        }
        if self.receiver.try_recv().is_ok() {
            self.stopped.store(true, Ordering::Release);
            return true;
        }
        false
    }

    /// Waits for the given duration or until shutdown is requested.
    ///
    /// Returns `true` if shutdown was requested, `false` if the duration elapsed.
    ///
    /// This is the preferred method for tasks that sleep between work cycles.
    pub fn wait(&self, duration: Duration) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return true;
        }
        match self.receiver.recv_timeout(duration) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.stopped.store(true, Ordering::Release);
                true
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
        }
    }
}

/// A long-running background task.
///
/// Implement this trait to define tasks that run in dedicated threads
/// alongside the main application loop. Tasks receive the event hub
/// to dispatch events and a shutdown signal for graceful termination.
pub trait BackgroundTask: Send {
    /// Returns the unique identifier for this task.
    fn id(&self) -> TaskId;

    /// Runs the task until shutdown is requested.
    ///
    /// This method is called in a dedicated thread. Use `hub` to send
    /// events to the main loop and `shutdown` to check for termination.
    fn run(&mut self, hub: &Sender<Event>, shutdown: &ShutdownSignal);

    /// Called when the task is being stopped.
    ///
    /// Override this to perform cleanup. The default implementation does nothing.
    fn stop(&mut self) {}
}

struct RunningTask {
    handle: JoinHandle<()>,
    shutdown: Sender<()>,
}

/// Manages the lifecycle of background tasks.
///
/// The task manager spawns tasks in dedicated threads and provides
/// methods to stop individual tasks or all tasks at once.
pub struct TaskManager {
    tasks: HashMap<TaskId, RunningTask>,
}

impl TaskManager {
    /// Creates a new empty task manager.
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Starts a background task in a new thread.
    ///
    /// The task receives a clone of `hub` for sending events and a
    /// [`ShutdownSignal`] for graceful termination.
    ///
    /// Returns an error if a task with the same ID is already running.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self, task, hub), fields(task_id = tracing::field::Empty), ret))]
    pub fn start(
        &mut self,
        task: Box<dyn BackgroundTask>,
        hub: Sender<Event>,
    ) -> Result<TaskId, TaskError> {
        let id = task.id();

        #[cfg(feature = "otel")]
        tracing::Span::current().record("task_id", tracing::field::display(&id));

        if self.is_running(&id) {
            return Err(TaskError::AlreadyRunning(id));
        }

        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let shutdown_signal = ShutdownSignal::new(shutdown_rx);

        let handle = thread::spawn(move || {
            let mut task = task;
            tracing::info!("task started");
            task.run(&hub, &shutdown_signal);
            task.stop();
            tracing::info!("task stopped");
        });

        self.tasks.insert(
            id.clone(),
            RunningTask {
                handle,
                shutdown: shutdown_tx,
            },
        );

        tracing::info!("task registered");
        Ok(id)
    }

    /// Stops a running task by ID.
    ///
    /// Sends the shutdown signal and waits for the task thread to finish.
    /// Returns an error if the task is not running.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(task_id = %id), ret))]
    pub fn stop(&mut self, id: &TaskId) -> Result<(), TaskError> {
        self.cleanup_finished();
        if let Some(task) = self.tasks.remove(id) {
            tracing::info!("sending shutdown signal");
            if let Err(e) = task.shutdown.send(()) {
                tracing::error!(error = %e, "failed to send shutdown signal");
            }
            if task.handle.join().is_err() {
                tracing::error!("task thread panicked");
            }
            Ok(())
        } else {
            Err(TaskError::NotRunning(id.clone()))
        }
    }

    /// Stops all running tasks.
    ///
    /// Sends shutdown signals to all tasks and waits for them to finish.
    #[cfg_attr(feature = "otel", tracing::instrument(skip(self), fields(task_count = tracing::field::Empty)))]
    pub fn stop_all(&mut self) {
        let tasks: Vec<_> = self.tasks.drain().collect();

        #[cfg(feature = "otel")]
        tracing::Span::current().record("task_count", tasks.len());

        if !tasks.is_empty() {
            tracing::info!("stopping all tasks");
        }
        for (_, task) in &tasks {
            if let Err(e) = task.shutdown.send(()) {
                tracing::error!(error = %e, "failed to send shutdown signal");
            }
        }
        for (_, task) in tasks {
            if task.handle.join().is_err() {
                tracing::error!("task thread panicked");
            }
        }
    }

    /// Removes entries for tasks whose threads have finished.
    fn cleanup_finished(&mut self) {
        self.tasks.retain(|_, task| !task.handle.is_finished());
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
pub fn register_startup_tasks(manager: &mut TaskManager, hub: Sender<Event>, _settings: &Settings) {
    #[cfg(feature = "kobo")]
    {
        let task = Box::new(wifi_status_monitor::WifiStatusMonitorTask);
        if let Err(e) = manager.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start wifi_status_monitor task");
        }
    }

    #[cfg(feature = "test")]
    {
        let settings = _settings;

        let task = Box::new(hello_world::HelloWorldTask);
        if let Err(e) = manager.start(task, hub.clone()) {
            tracing::warn!(error = %e, "failed to start hello_world task");
        }

        #[cfg(feature = "kobo")]
        if settings.logging.enable_dbus_log {
            let task = Box::new(dbus_monitor::DbusMonitorTask);
            if let Err(e) = manager.start(task, hub) {
                tracing::warn!(error = %e, "failed to start dbus_monitor task");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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

        fn run(&mut self, _hub: &Sender<Event>, _shutdown: &ShutdownSignal) {}
    }

    struct WaitingTask;

    impl BackgroundTask for WaitingTask {
        fn id(&self) -> TaskId {
            TaskId::TestTask
        }

        fn run(&mut self, _hub: &Sender<Event>, shutdown: &ShutdownSignal) {
            shutdown.wait(Duration::from_secs(60));
        }
    }

    #[test]
    fn start_and_stop() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        let id = manager.start(Box::new(WaitingTask), hub).unwrap();
        assert!(manager.is_running(&id));

        manager.stop(&id).unwrap();
        assert!(!manager.is_running(&id));
    }

    #[test]
    fn duplicate_start_returns_error() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        manager.start(Box::new(WaitingTask), hub.clone()).unwrap();
        let err = manager.start(Box::new(WaitingTask), hub).unwrap_err();

        assert!(matches!(err, TaskError::AlreadyRunning(TaskId::TestTask)));
    }

    #[test]
    fn finished_task_is_cleaned_up() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        let id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &id);
        assert!(!manager.is_running(&id));
    }

    #[test]
    fn stop_finished_task_returns_not_running() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        let id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &id);
        let err = manager.stop(&id).unwrap_err();

        assert!(matches!(err, TaskError::NotRunning(TaskId::TestTask2)));
    }

    #[test]
    fn running_tasks_excludes_finished() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        manager.start(Box::new(WaitingTask), hub.clone()).unwrap();
        let instant_id = manager.start(Box::new(InstantTask), hub).unwrap();

        wait_until_not_running(&mut manager, &instant_id);
        let running = manager.running_tasks();

        assert_eq!(running.len(), 1);
        assert_eq!(running[0], TaskId::TestTask);

        manager.stop_all();
    }

    #[test]
    fn stop_all_stops_everything() {
        let mut manager = TaskManager::new();
        let (hub, _rx) = mpsc::channel();

        manager.start(Box::new(WaitingTask), hub).unwrap();
        manager.stop_all();

        assert!(!manager.is_running(&TaskId::TestTask));
    }
}
