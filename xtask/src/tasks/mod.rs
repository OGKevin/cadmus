//! Task implementations for `cargo xtask`.
//!
//! Each sub-module corresponds to one top-level subcommand.  Shared utilities
//! live in [`util`].

pub mod bench;
pub mod build_kobo;
pub mod bundle;
pub mod ci;
pub mod clippy;
pub mod dist;
pub mod docs;
pub mod download_assets;
pub mod fmt;
pub mod install_importer;
pub mod run_emulator;
pub mod setup_native;
pub mod test;
pub mod util;
