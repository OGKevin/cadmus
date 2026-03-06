//! GitHub API client and device flow authentication.
//!
//! This module provides:
//! - [`GithubClient`] — a thin blocking HTTP wrapper for the GitHub REST API
//! - [`device_flow`] — token persistence helpers (`save_token`, `load_token`)
//! - Shared types used by both the client and callers

mod client;
pub mod device_flow;
pub(crate) mod types;

pub use client::CLIENT_TIMEOUT_SECS;
pub use client::{GithubClient, REQUIRED_SCOPES};
pub use types::{DeviceCodeResponse, OtaProgress, ScopeError, TokenPollResult, VerifyScopesError};
