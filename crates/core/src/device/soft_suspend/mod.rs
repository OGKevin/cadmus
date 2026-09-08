//! Soft-suspend settings contract and mode types.
//!
//! [`SoftSuspendBackend`] is the settings / diagnostics surface used by the
//! power settings UI and suspend orchestrator. Production settings go through
//! [`Inhibitor`](crate::device::inhibitor::Inhibitor), which implements this
//! trait. Lease **acquire** is not on this trait — use
//! [`Inhibitor::acquire`](crate::device::inhibitor::Inhibitor::acquire) with
//! [`Kind::SoftSuspend`](crate::device::inhibitor::Kind::SoftSuspend).
//!
//! Linux SoftSuspend-kind backends are built by device probe and passed into
//! [`Inhibitor::new`](crate::device::inhibitor::Inhibitor::new).

pub(crate) mod backend;
pub mod mode;

pub use backend::SoftSuspendBackend;
