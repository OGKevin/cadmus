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
    AddLibraryRequest, LibraryValue, ListSettingsRequest, RemoveLibraryRequest, Setting, SettingId,
    SettingValue, Settings, SettingsCatalog, UpdateSettingRequest,
};
use catalog::{list, remove_library, update};
use connectrpc::{ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult};
use libraries::add_library;
use std::future::Future;
use tokio::sync::oneshot;

#[cfg(test)]
pub(super) use values::{checks_value, choice_value, library_value, path_value, toggle_value};

/// Settings work a listener asks the main loop to do.
pub(super) enum Command {
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
    /// Append a library and return its row.
    AddLibrary {
        library: LibraryValue,
        reply: oneshot::Sender<Result<Setting, ConnectError>>,
    },
    /// Remove a library addressed by `id`.
    RemoveLibrary {
        id: SettingId,
        reply: oneshot::Sender<Result<(), ConnectError>>,
    },
}

impl Command {
    /// True when the caller stopped waiting for the reply.
    ///
    /// The main loop runs on the device thread, so a command whose requester
    /// timed out must not touch settings or the editor.
    pub(super) fn reply_is_closed(&self) -> bool {
        match self {
            Self::List { reply } => reply.is_closed(),
            Self::Update { reply, .. } => reply.is_closed(),
            Self::AddLibrary { reply, .. } => reply.is_closed(),
            Self::RemoveLibrary { reply, .. } => reply.is_closed(),
        }
    }
}

/// Why mutating RPCs fail while the device storage is shared over USB.
const STORAGE_SHARED: &str = "device storage is shared over USB";

/// Connected clients get this instead of a settings write during USB share.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn unavailable_shared() -> ConnectError {
    ConnectError::unavailable(STORAGE_SHARED)
}

/// Drain one List or Update on the main loop.
///
/// USB mass storage unmounts `/mnt/onboard`, so a settings write would land on
/// the rootfs mountpoint; mutating commands fail while the storage is shared.
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
            let result = if context.shared {
                Err(unavailable_shared())
            } else {
                update(&id, &value, hub, bus, context, runtime).await
            };
            let _ = reply.send(result);
        }
        Command::AddLibrary { library, reply } => {
            let result = if context.shared {
                Err(unavailable_shared())
            } else {
                add_library(&library, hub, context, runtime).await
            };
            let _ = reply.send(result);
        }
        Command::RemoveLibrary { id, reply } => {
            let result = if context.shared {
                Err(unavailable_shared())
            } else {
                remove_library(&id, hub, context, runtime).await
            };
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

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn add_library<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, AddLibraryRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<Setting> + Send + use<'a>>> + Send {
        let owned = request.to_owned_message();
        let library = owned.library.into_option().unwrap_or_default();
        let tx = self.tx.clone();
        let hub = self.hub.clone();
        let inhibitor = std::sync::Arc::clone(&self.inhibitor);
        async move {
            ask(&tx, &hub, &inhibitor, |reply| {
                super::Command::Settings(Command::AddLibrary { library, reply })
            })
            .await
            .and_then(connectrpc::Response::ok)
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn remove_library<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, RemoveLibraryRequest>,
    ) -> impl Future<
        Output = ServiceResult<
            impl Encodable<buffa_types::google::protobuf::Empty> + Send + use<'a>,
        >,
    > + Send {
        let owned = request.to_owned_message();
        let id = owned.id.into_option().unwrap_or_default();
        let tx = self.tx.clone();
        let hub = self.hub.clone();
        let inhibitor = std::sync::Arc::clone(&self.inhibitor);
        async move {
            ask(&tx, &hub, &inhibitor, |reply| {
                super::Command::Settings(Command::RemoveLibrary { id, reply })
            })
            .await
            .and_then(|()| {
                connectrpc::Response::ok(buffa_types::google::protobuf::Empty::default())
            })
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
