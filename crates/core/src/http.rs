//! Reusable HTTP client with pre-configured TLS, timeouts, and user agent.
//!
//! This module provides [`Client`] as the recommended base HTTP client for all
//! network requests in the application. It is pre-configured with:
//!
//! - TLS using `webpki-roots` certificates (no system cert store required)
//! - 30 second request timeout
//! - User agent identifying the application
//!
//! # Example
//!
//! ```no_run
//! use cadmus_core::http::Client;
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::new()?;
//!     client.get("https://example.com").send()?;
//!     Ok(())
//! }
//! ```

use reqwest::blocking::{Client as ReqwestClient, RequestBuilder};
use rustls::RootCertStore;
use std::time::Duration;
use thiserror::Error;

pub const CLIENT_TIMEOUT_SECS: u64 = 30;

const USER_AGENT: &str = concat!("github.com/OGKevin/cadmus/", env!("GIT_VERSION"));

#[derive(Error, Debug)]
pub enum HttpError {
    #[error("Failed to build HTTP client: {0}")]
    Build(#[from] reqwest::Error),
}

/// Pre-configured HTTP client for making network requests.
///
/// This client should be used as the base for all HTTP requests rather than
/// constructing raw `reqwest` clients. It comes with:
/// - TLS using `webpki-roots` certificates (works on Kobo devices without system cert store)
/// - 30 second request timeout
/// - User agent header set
///
/// # Example
///
/// ```no_run
/// use cadmus_core::http::Client;
///
/// fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let client = Client::new()?;
///     client.get("https://api.github.com").send()?;
///     Ok(())
/// }
/// ```
pub struct Client {
    client: ReqwestClient,
}

impl Client {
    pub fn new() -> Result<Self, HttpError> {
        let root_store = build_root_store();

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        let client = ReqwestClient::builder()
            .use_preconfigured_tls(tls_config)
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(CLIENT_TIMEOUT_SECS))
            .build()
            .map_err(HttpError::Build)?;

        tracing::debug!("HTTP client built successfully");
        Ok(Self { client })
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.client.get(url)
    }

    pub fn post(&self, url: &str) -> RequestBuilder {
        self.client.post(url)
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
        }
    }
}

fn build_root_store() -> RootCertStore {
    let mut store = RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    store
}
