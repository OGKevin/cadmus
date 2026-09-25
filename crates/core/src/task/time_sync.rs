use std::sync::{Arc, Mutex};

use crate::device::rtc::{AlarmManager, Rtc};
use crate::device::wifi::WifiSession;
use crate::geolocation::fetch_geolocation;
use crate::http::Client;
use crate::network_address::NetworkAddress;
use crate::task::{BackgroundTask, TaskFuture, TaskId};
use crate::time_manager::TimeManager;
use crate::view::Event;
use tokio_util::sync::CancellationToken;

pub struct TimeSyncTask<R: Rtc> {
    time_manager: TimeManager<R>,
    ntp_server: NetworkAddress,
    manual: bool,
    wifi_session: Arc<WifiSession>,
    alarm_manager: Arc<Mutex<AlarmManager<R>>>,
}

impl<R: Rtc> TimeSyncTask<R> {
    pub fn new(
        time_manager: TimeManager<R>,
        ntp_server: NetworkAddress,
        manual: bool,
        wifi_session: Arc<WifiSession>,
        alarm_manager: Arc<Mutex<AlarmManager<R>>>,
    ) -> Self {
        TimeSyncTask {
            time_manager,
            ntp_server,
            manual,
            wifi_session,
            alarm_manager,
        }
    }
}

impl<R: Rtc + Send + 'static> BackgroundTask for TimeSyncTask<R> {
    fn id(&self) -> TaskId {
        TaskId::TimeSync
    }

    /// Synchronises the clock, checking cancellation between the same steps
    /// master used (`should_stop` after lease, geolocation, and before apply).
    fn run<'a>(
        &'a mut self,
        hub: &'a crate::view::Hub,
        cancel: &'a CancellationToken,
    ) -> TaskFuture<'a> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return;
            }

            let _wifi = match self.wifi_session.acquire("time-sync").await {
                Ok(lease) => lease,
                Err(e) => {
                    tracing::error!(error = %e, "failed to acquire WiFi lease for time sync");
                    if self.manual {
                        hub.send(
                            (Event::Notification(crate::view::NotificationEvent::Show(
                                crate::fl!("notification-time-sync-failed"),
                            )))
                            .into(),
                        )
                        .ok();
                    }
                    return;
                }
            };

            if cancel.is_cancelled() {
                return;
            }

            let geo = match Client::new() {
                Ok(client) => match fetch_geolocation(&client).await {
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

            if cancel.is_cancelled() {
                return;
            }

            let coordinates = geo.as_ref().map(|geo| geo.coordinates);

            if let Err(e) = self
                .time_manager
                .sync(&self.ntp_server, self.manual, geo, hub, &self.alarm_manager)
                .await
            {
                tracing::error!(error = %e, "time sync failed");
            }

            if cancel.is_cancelled() {
                return;
            }

            if let Some(coordinates) = coordinates {
                hub.send((Event::AutoFrontlightCoordinates(coordinates)).into())
                    .ok();
            }
        })
    }
}
