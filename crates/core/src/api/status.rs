//! Shared Status RPCs.

use super::handler::{PublicRpc, ask};
use crate::device::AppContext;
use crate::version::BuildKind;
use cadmus_api_proto::cadmus::api::shared::v1::{
    DeviceStatus, GetStatusRequest, Locale, Status, Version,
};
use connectrpc::{ConnectError, Encodable, RequestContext, ServiceRequest, ServiceResult};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Status work a listener asks the main loop to do.
pub(crate) enum Command {
    /// Locale, hostname, and compile-time version.
    Get {
        reply: oneshot::Sender<Result<DeviceStatus, ConnectError>>,
    },
}

impl Status for PublicRpc {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn get_status<'a>(
        &'a self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetStatusRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<DeviceStatus> + Send + use<'a>>> + Send
    {
        let _ = request.to_owned_message();
        let tx = self.tx.clone();
        let hub = self.hub.clone();
        let inhibitor = Arc::clone(&self.inhibitor);
        async move {
            ask(&tx, &hub, &inhibitor, |reply| {
                super::Command::Status(Command::Get { reply })
            })
            .await
            .and_then(connectrpc::Response::ok)
        }
    }
}

/// Locale from the public listener if it is up, otherwise English. Version from get_version.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn current(context: &AppContext) -> DeviceStatus {
    let locale = context
        .api
        .public
        .as_ref()
        .map(|running| running.locale)
        .unwrap_or(Locale::LOCALE_EN);
    let version = crate::version::get_version();
    let git = version.git();
    DeviceStatus {
        locale: locale.into(),
        hostname: context.mdns.hostname(),
        version: Version {
            major: git.major(),
            minor: git.minor(),
            patch: git.patch(),
            commits_ahead: git.commits_ahead(),
            commit: git.hash().unwrap_or_default().to_string(),
            dirty: git.is_dirty(),
            pull_request: version
                .pull_request()
                .map(|pr| pr.as_str().to_string())
                .unwrap_or_default(),
            build_user: version.build().user.to_string(),
            build_host: version.build().host.to_string(),
            build_timestamp: version
                .build()
                .timestamp
                .unix_seconds()
                .map(|seconds| buffa_types::google::protobuf::Timestamp {
                    seconds,
                    ..Default::default()
                })
                .into(),
            test: matches!(version.build_kind(), BuildKind::Test),
            ..Default::default()
        }
        .into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::current;
    use crate::context::test_helpers::create_test_context;
    use cadmus_api_proto::cadmus::api::shared::v1::Locale;

    #[tokio::test]
    async fn current_uses_english_locale_without_public_listener() {
        let context = create_test_context().await;
        let status = current(&context);
        assert_eq!(status.locale.as_known(), Some(Locale::LOCALE_EN));
        assert!(!status.hostname.is_empty());
        assert!(!status.version.commit.is_empty() || status.version.major > 0);
    }
}
