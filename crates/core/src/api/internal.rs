//! Loopback-only `cadmus.api.internal.v1` RPCs.
//!
//! Registered by [`super::serve::listen`] only when the listener scope is
//! private, so the public companion listener cannot route them.

use super::handler::PublicRpc;
use cadmus_api_proto::cadmus::api::internal::v1::{
    PingRequest, PingResponse, Placeholder, PlaceholderExt,
};
use connectrpc::{Encodable, RequestContext, Router, ServiceRequest, ServiceResult};
use std::future::Future;
use std::ops::Deref;
use std::sync::Arc;

/// Loopback-only handler.
///
/// Wraps [`PublicRpc`] so private RPCs share the command channel, hub, and
/// inhibitor instead of duplicating them. Deref reaches the shared plumbing.
pub(super) struct InternalRpc {
    public: Arc<PublicRpc>,
}

impl Deref for InternalRpc {
    type Target = PublicRpc;

    fn deref(&self) -> &Self::Target {
        &self.public
    }
}

/// Add the private services to `router`.
pub(super) fn register(handler: &Arc<PublicRpc>, router: Router) -> Router {
    let internal = Arc::new(InternalRpc {
        public: Arc::clone(handler),
    });
    PlaceholderExt::register(internal, router)
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
