use std::sync::Arc;
use std::sync::mpsc::Sender;

use crate::device::rtc::Rtc;
use crate::device::wifi::WifiSession;
use crate::geolocation::fetch_geolocation;
use crate::http::Client;
use crate::network_address::NetworkAddress;
use crate::task::{BackgroundTask, ShutdownSignal, TaskId};
use crate::time_manager::TimeManager;
use crate::view::Event;

pub struct TimeSyncTask<R: Rtc> {
    time_manager: TimeManager<R>,
    ntp_server: NetworkAddress,
    manual: bool,
    wifi_session: Arc<WifiSession>,
}

impl<R: Rtc> TimeSyncTask<R> {
    pub fn new(
        time_manager: TimeManager<R>,
        ntp_server: NetworkAddress,
        manual: bool,
        wifi_session: Arc<WifiSession>,
    ) -> Self {
        TimeSyncTask {
            time_manager,
            ntp_server,
            manual,
            wifi_session,
        }
    }
}

impl<R: Rtc + Send + 'static> BackgroundTask for TimeSyncTask<R> {
    fn id(&self) -> TaskId {
        TaskId::TimeSync
    }

    fn run(&mut self, hub: &Sender<Event>, _shutdown: &ShutdownSignal) {
        let _wifi = match self.wifi_session.acquire("time-sync") {
            Ok(lease) => lease,
            Err(e) => {
                tracing::error!(error = %e, "failed to acquire WiFi lease for time sync");
                if self.manual {
                    hub.send(Event::Notification(crate::view::NotificationEvent::Show(
                        crate::fl!("notification-time-sync-failed"),
                    )))
                    .ok();
                }
                return;
            }
        };

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

        if let Err(e) = self
            .time_manager
            .sync(&self.ntp_server, self.manual, geo, hub)
        {
            tracing::error!(error = %e, "time sync failed");
        }

        if let Some(coordinates) = coordinates {
            hub.send(Event::AutoFrontlightCoordinates(coordinates)).ok();
        }
    }
}
