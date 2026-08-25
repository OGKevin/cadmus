//! GitHub API client and device flow authentication.
//!
//! This module provides:
//! - [`GithubClient`] — a thin blocking HTTP wrapper for the GitHub REST API
//! - [`device_flow`] — token persistence helpers ([`device_flow::save_token`],
//!   [`device_flow::load_token`], [`device_flow::resolve_auth_token`]) and
//!   debug-build [`device_flow::token_from_env`] resolution
//! - Shared types used by both the client and callers

mod client;
pub mod device_flow;
pub(crate) mod types;

pub use crate::http::CLIENT_TIMEOUT_SECS;
pub use client::GithubClient;
pub use client::REQUIRED_SCOPES;
pub use types::{
    DeviceCodeResponse, GithubError, OtaProgress, ScopeError, TokenPollResult, VerifyScopesError,
};
