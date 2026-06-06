use std::sync::mpsc::Sender;

use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::time_manager::TimeManager;
use crate::view::Event;

const NTP_HOST: &str = "time.cloudflare.com:123";

pub struct TimeSyncTask {
    time_manager: &'static TimeManager,
    manual: bool,
}

impl TimeSyncTask {
    pub fn new(time_manager: &'static TimeManager, manual: bool) -> Self {
        TimeSyncTask {
            time_manager,
            manual,
        }
    }
}

impl BackgroundTask for TimeSyncTask {
    fn id(&self) -> TaskId {
        TaskId::TimeSync
    }

    fn run(&mut self, hub: &Sender<Event>, _shutdown: &ShutdownSignal) {
        if let Err(e) = self.time_manager.sync(NTP_HOST, self.manual, hub) {
            tracing::error!(error = %e, "time sync failed");
        }
    }
}
