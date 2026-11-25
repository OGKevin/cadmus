//! Loopback-only `cadmus.api.internal.v1` RPCs.
//!
//! Registered by [`super::private::register`] on the loopback listener only,
//! so the public companion listener cannot route them.

use cadmus_api_proto::cadmus::api::internal::v1::{
    PingRequest, PingResponse, Placeholder, PlaceholderExt,
};
use connectrpc::{Encodable, RequestContext, Router, ServiceRequest, ServiceResult};
use std::future::Future;
use std::sync::Arc;

/// Loopback-only handler: the `cadmus.api.internal.v1` services.
///
/// Internal-only state lives here, apart from the shared services in
/// [`super::shared_rpc::SharedRpc`].
pub(super) struct InternalRpc;

/// Add the private services to `router`.
pub(super) fn register(router: Router) -> Router {
    PlaceholderExt::register(Arc::new(InternalRpc), router)
}

impl Placeholder for InternalRpc {
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    fn ping<'a>(
        &'a self,
        _ctx: RequestContext,
        _request: ServiceRequest<'_, PingRequest>,
    ) -> impl Future<Output = ServiceResult<impl Encodable<PingResponse> + Send + use<'a>>> + Send
    {
        std::future::ready(connectrpc::Response::ok(PingResponse::default()))
    }
}
