//! Settings catalog and Settings RPCs.

mod catalog;
mod ident;
mod intermission;
mod libraries;
mod refresh;
mod values;

use super::handler::{PublicRpc, ask};
use crate::device::{AppContext, DeviceRuntime};
use crate::view::{Bus, Hub};
use cadmus_api_proto::cadmus::api::shared::v1::{
    ListSettingsRequest, Setting, SettingId, SettingValue, Settings, SettingsCatalog,
    UpdateSettingRequest,
};
use catalog::{list, update};
use connectrpc::{ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult};
use std::future::Future;
use tokio::sync::oneshot;

#[cfg(test)]
pub(crate) use values::{checks_value, choice_value, library_value, path_value, toggle_value};

/// Settings work a listener asks the main loop to do.
pub(crate) enum Command {
    /// Return the companion settings catalog.
    List {
        reply: oneshot::Sender<Result<SettingsCatalog, ConnectError>>,
    },
    /// Apply one setting change on the main loop.
    Update {
        id: SettingId,
        value: SettingValue,
        reply: oneshot::Sender<Result<Setting, ConnectError>>,
    },
}

/// Drain one List or Update on the main loop.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn handle(
    command: Command,
    hub: &Hub,
    bus: &mut Bus,
    context: &mut AppContext,
    runtime: &mut DeviceRuntime<'_>,
) {
    match command {
        Command::List { reply } => {
            let catalog = list(context).await;
            let _ = reply.send(Ok(catalog));
        }
        Command::Update { id, value, reply } => {
            let result = update(&id, &value, hub, bus, context, runtime).await;
            let _ = reply.send(result);
        }
    }
}

impl Settings for PublicRpc {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn list<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ListSettingsRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<SettingsCatalog> + Send + use<'a>>> + Send
    {
        let _ = request.to_owned_message();
        let tx = self.tx.clone();
        let hub = self.hub.clone();
        let inhibitor = std::sync::Arc::clone(&self.inhibitor);
        async move {
            ask(&tx, &hub, &inhibitor, |reply| {
                super::Command::Settings(Command::List { reply })
            })
            .await
            .and_then(connectrpc::Response::ok)
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn update<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, UpdateSettingRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<Setting> + Send + use<'a>>> + Send {
        let owned = request.to_owned_message();
        let tx = self.tx.clone();
        let hub = self.hub.clone();
        let inhibitor = std::sync::Arc::clone(&self.inhibitor);
        let id = owned.id.into_option().unwrap_or_default();
        let value = owned.value.into_option().unwrap_or_default();
        async move {
            ask(&tx, &hub, &inhibitor, |reply| {
                super::Command::Settings(Command::Update { id, value, reply })
            })
            .await
            .and_then(connectrpc::Response::ok)
        }
    }
}

/// Writes settings after an API change. Missing manager is a no-op.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) async fn save(context: &AppContext, runtime: &DeviceRuntime<'_>) {
    let Some(manager) = runtime.settings_manager else {
        return;
    };
    if let Err(error) = manager.save(&context.settings).await {
        tracing::error!(error = %error, "api settings save failed");
    }
}
