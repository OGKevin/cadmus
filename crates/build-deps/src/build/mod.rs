//! Build orchestration for the thirdparty C/C++ libraries.
//!
//! Two submodules cover the two build targets Cadmus supports:
//!
//! * [`kobo`] cross-compiles the libraries for the Kobo e-reader's
//!   ARM CPU using the Linaro toolchain.
//! * [`native`] builds MuPDF and libwebp for the Linux/macOS host so
//!   the rest of the workspace can run unit tests and the emulator
//!   without cross-compilation.
//!
//! The [`mupdf`] module holds the cross-flow MuPDF source
//! preparation (WebP support patches) shared by both [`kobo`] and
//! [`native`].
//!
//! The [`mupdf_wrapper`] module compiles the small C glue library
//! (`mupdf_wrapper.c`) that exposes a curated subset of MuPDF entry
//! points to Rust for both build flows.

pub mod kobo;
pub mod mupdf;
pub mod mupdf_wrapper;
pub mod native;
pub mod sqlite;
