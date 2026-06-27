use std::sync::mpsc::Sender;

use crate::device::rtc::Rtc;
use crate::geolocation::fetch_geolocation;
use crate::http::Client;
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::time_manager::TimeManager;
use crate::view::Event;

const NTP_HOST: &str = "time.cloudflare.com:123";

pub struct TimeSyncTask<R: Rtc> {
    time_manager: TimeManager<R>,
    manual: bool,
}

impl<R: Rtc> TimeSyncTask<R> {
    pub fn new(time_manager: TimeManager<R>, manual: bool) -> Self {
        TimeSyncTask {
            time_manager,
            manual,
        }
    }
}

impl<R: Rtc + Send + 'static> BackgroundTask for TimeSyncTask<R> {
    fn id(&self) -> TaskId {
        TaskId::TimeSync
    }

    fn run(&mut self, hub: &Sender<Event>, _shutdown: &ShutdownSignal) {
        let geo = match Client::new() {
            Ok(client) => match fetch_geolocation(&client) {
                Ok(geo) => Some(geo),
                Err(e) => {
                    tracing::error!(error = %e, "failed to fetch geolocation");
                    None
                }
            },
            Err(e) => {
                tracing::error!(error = %e, "failed to create http client");
                None
            }
        };

        let coordinates = geo.as_ref().map(|geo| geo.coordinates);

        if let Err(e) = self.time_manager.sync(NTP_HOST, self.manual, geo, hub) {
            tracing::error!(error = %e, "time sync failed");
        }

        if let Some(coordinates) = coordinates {
            hub.send(Event::AutoFrontlightCoordinates(coordinates)).ok();
        }
    }
}
