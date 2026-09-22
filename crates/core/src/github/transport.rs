//! Leaf [`tower::Service`] that sends `octocrab` requests through Cadmus's
//! middleware HTTP client.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::combinators::BoxBody;
use octocrab::OctoBody;
use reqwest_middleware::ClientWithMiddleware;
use tower::Service;

#[derive(Debug, thiserror::Error)]
pub(crate) enum TransportError {
    #[error(transparent)]
    Middleware(#[from] reqwest_middleware::Error),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error("{0}")]
    Body(String),
}

pub(crate) type ResponseBody = BoxBody<Bytes, TransportError>;

#[derive(Clone)]
pub(crate) struct CadmusHttpService {
    client: ClientWithMiddleware,
}

impl CadmusHttpService {
    pub(crate) fn new(client: ClientWithMiddleware) -> Self {
        Self { client }
    }
}

impl Service<http::Request<OctoBody>> for CadmusHttpService {
    type Response = http::Response<ResponseBody>;
    type Error = TransportError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: http::Request<OctoBody>) -> Self::Future {
        let client = self.client.clone();
        Box::pin(async move {
            let (parts, body) = request.into_parts();
            let bytes = body
                .collect()
                .await
                .map_err(|error| TransportError::Body(error.to_string()))?
                .to_bytes();
            let request = http::Request::from_parts(parts, reqwest::Body::from(bytes));
            let request = reqwest::Request::try_from(request)?;
            let response = client.execute(request).await?;
            let (parts, body) = http::Response::<reqwest::Body>::from(response).into_parts();
            let body = body
                .map_err(|error| TransportError::Body(error.to_string()))
                .boxed();
            Ok(http::Response::from_parts(parts, body))
        })
    }
}
