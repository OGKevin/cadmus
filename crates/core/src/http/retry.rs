//! Transient-request retries with exponential backoff and `Retry-After` honouring.
//!
//! Middleware retries cover transport failures and retryable statuses after
//! response headers arrive (`send()`). They do **not** retry mid-body read
//! failures on chunked downloads — that needs an app-level loop or streaming
//! download (see [`super::Client::download`]).

use anyhow::anyhow;
use http::Extensions;
use reqwest::{Request, Response};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, Error, Middleware, Next, Result};
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::{
    Jitter, RetryDecision, RetryError, RetryPolicy, Retryable, RetryableStrategy,
    default_on_request_failure, default_on_request_success,
};
use std::time::{Duration, SystemTime};

use super::RETRY_BACKOFFS;

pub(super) fn retry_policy() -> ExponentialBackoff {
    ExponentialBackoff::builder()
        .retry_bounds(Duration::from_secs(1), Duration::from_secs(20))
        .jitter(Jitter::None)
        .base(2)
        .build_with_max_retries(u32::try_from(RETRY_BACKOFFS).unwrap_or(0))
}

pub(super) fn client_with_retry(raw: reqwest::Client) -> ClientWithMiddleware {
    attach_request_spans(
        ClientBuilder::new(raw).with(RetryTransientWithRetryAfter::new(retry_policy())),
    )
    .build()
}

fn attach_request_spans(builder: ClientBuilder) -> ClientBuilder {
    #[cfg(feature = "tracing")]
    {
        builder.with(reqwest_tracing::TracingMiddleware::default())
    }
    #[cfg(not(feature = "tracing"))]
    {
        builder
    }
}

/// Caps `Retry-After` so a hostile or misconfigured server cannot stall the
/// client for minutes on each attempt.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(20);

/// Parses a `Retry-After` header value as delay-seconds (HTTP-date forms are
/// ignored and treated as absent).
pub(super) fn retry_after_delay(response: &Response) -> Option<Duration> {
    let raw = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    let secs: u64 = raw.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

struct DefaultStrategy;

impl RetryableStrategy for DefaultStrategy {
    fn handle(&self, res: &Result<Response>) -> Option<Retryable> {
        match res {
            Ok(success) => default_on_request_success(success),
            Err(error) => default_on_request_failure(error),
        }
    }
}

/// Like [`reqwest_retry::RetryTransientMiddleware`], but sleeps at least as long
/// as a successful response's `Retry-After` when present (e.g. HTTP 429).
struct RetryTransientWithRetryAfter<T> {
    retry_policy: T,
    strategy: DefaultStrategy,
}

impl<T: RetryPolicy + Send + Sync> RetryTransientWithRetryAfter<T> {
    fn new(retry_policy: T) -> Self {
        Self {
            retry_policy,
            strategy: DefaultStrategy,
        }
    }
}

#[async_trait::async_trait]
impl<T> Middleware for RetryTransientWithRetryAfter<T>
where
    T: RetryPolicy + Send + Sync + 'static,
{
    async fn handle(
        &self,
        req: Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> Result<Response> {
        let mut n_past_retries = 0;
        let start_time = SystemTime::now();
        loop {
            let duplicate_request = req.try_clone().ok_or_else(|| {
                Error::Middleware(anyhow!(
                    "Request object is not cloneable. Are you passing a streaming body?"
                ))
            })?;

            let result = next.clone().run(duplicate_request, extensions).await;

            if let Some(Retryable::Transient) = self.strategy.handle(&result) {
                let retry_after = result.as_ref().ok().and_then(retry_after_delay);
                let retry_decision = self.retry_policy.should_retry(start_time, n_past_retries);
                if let RetryDecision::Retry { execute_after } = retry_decision {
                    let policy_delay = execute_after
                        .duration_since(SystemTime::now())
                        .unwrap_or_default();
                    let duration = match retry_after {
                        Some(after) => policy_delay.max(after),
                        None => policy_delay,
                    };
                    tracing::debug!(
                        n_past_retries,
                        ?duration,
                        ?retry_after,
                        "Retrying transient HTTP failure"
                    );
                    tokio::time::sleep(duration).await;
                    n_past_retries += 1;
                    continue;
                }
            }

            break if n_past_retries > 0 {
                result.map_err(|err| {
                    Error::Middleware(
                        RetryError::WithRetries {
                            retries: n_past_retries,
                            err,
                        }
                        .into(),
                    )
                })
            } else {
                result.map_err(|err| Error::Middleware(RetryError::Error(err).into()))
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;

    #[test]
    fn retry_policy_allows_two_backoffs() {
        let policy = retry_policy();
        let start = SystemTime::now();
        assert!(matches!(
            policy.should_retry(start, 0),
            RetryDecision::Retry { .. }
        ));
        assert!(matches!(
            policy.should_retry(start, 1),
            RetryDecision::Retry { .. }
        ));
        assert!(matches!(
            policy.should_retry(start, 2),
            RetryDecision::DoNotRetry
        ));
    }

    #[test]
    fn retry_after_parses_delay_seconds() {
        let response = Response::from(
            http::Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(reqwest::header::RETRY_AFTER, "5")
                .body("")
                .unwrap(),
        );
        assert_eq!(retry_after_delay(&response), Some(Duration::from_secs(5)));
    }

    #[test]
    fn retry_after_caps_large_values() {
        let response = Response::from(
            http::Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(reqwest::header::RETRY_AFTER, "9999")
                .body("")
                .unwrap(),
        );
        assert_eq!(retry_after_delay(&response), Some(MAX_RETRY_AFTER));
    }

    #[test]
    fn retry_after_ignores_http_date() {
        let response = Response::from(
            http::Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header(
                    reqwest::header::RETRY_AFTER,
                    "Wed, 21 Oct 2015 07:28:00 GMT",
                )
                .body("")
                .unwrap(),
        );
        assert_eq!(retry_after_delay(&response), None);
    }
}
