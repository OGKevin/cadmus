//! Named RAII resource leases.
//!
//! A [`LeaseTracker`] tracks named holders of a shared resource. Each
//! [`acquire`](LeaseTracker::acquire) returns a [`Lease`] guard; dropping the
//! guard releases that holder. When the last holder drops, observers can react
//! (for example arming an idle timer).
//!
//! Prefer [`LeaseTracker::with`] or the [`lease`] attribute when holding a
//! lease for a whole function body.
//!
//! # Examples
//!
//! ```
//! use cadmus_core::lease::LeaseTracker;
//!
//! let tracker = LeaseTracker::new();
//!
//! let lease = tracker.acquire("time-sync");
//! assert_eq!(tracker.len(), 1);
//! drop(lease);
//! assert!(tracker.is_empty());
//!
//! let value = tracker.with("ota-download", || 42);
//! assert_eq!(value, 42);
//! assert!(tracker.is_empty());
//! ```

pub use cadmus_macros::lease;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Display name for a lease holder, used in logs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LeaseName(Cow<'static, str>);

impl LeaseName {
    /// Creates a lease name from a static string.
    pub fn new(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Returns the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for LeaseName {
    fn from(value: &'static str) -> Self {
        Self::new(value)
    }
}

impl From<String> for LeaseName {
    fn from(value: String) -> Self {
        Self(Cow::Owned(value))
    }
}

impl fmt::Display for LeaseName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Opaque unique id for one acquire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeaseId(u64);

impl LeaseId {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }

    /// Returns the raw id value.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for LeaseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Observer notified when holder count transitions between empty and non-empty.
///
/// Attach one with [`LeaseTracker::with_observer`]. Callbacks run while the
/// tracker lock is **not** held, so they may acquire leases on the same
/// tracker if needed.
///
/// # Examples
///
/// Enable a resource on the first holder and tear it down when the last
/// holder drops:
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
///
/// use cadmus_core::lease::{LeaseName, LeaseObserver, LeaseTracker};
///
/// struct ResourceGate {
///     active: AtomicBool,
/// }
///
/// impl LeaseObserver for ResourceGate {
///     fn on_first_acquire(&self, _name: &LeaseName) {
///         self.active.store(true, Ordering::SeqCst);
///     }
///
///     fn on_last_release(&self, _name: &LeaseName) {
///         self.active.store(false, Ordering::SeqCst);
///     }
/// }
///
/// let gate = Arc::new(ResourceGate {
///     active: AtomicBool::new(false),
/// });
/// let tracker = LeaseTracker::with_observer(gate.clone());
///
/// let lease = tracker.acquire("worker");
/// assert!(gate.active.load(Ordering::SeqCst));
/// drop(lease);
/// assert!(!gate.active.load(Ordering::SeqCst));
/// ```
pub trait LeaseObserver: Send + Sync {
    /// Called when the first holder is acquired (0 → 1).
    fn on_first_acquire(&self, name: &LeaseName);

    /// Called when the last holder is released (1 → 0).
    fn on_last_release(&self, name: &LeaseName);
}

struct TrackerState {
    holders: HashMap<LeaseId, LeaseName>,
}

/// Shared tracker for named resource leases.
#[derive(Clone)]
pub struct LeaseTracker {
    inner: Arc<TrackerInner>,
}

struct TrackerInner {
    state: Mutex<TrackerState>,
    observer: Option<Arc<dyn LeaseObserver>>,
}

impl fmt::Debug for LeaseTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        f.debug_struct("LeaseTracker")
            .field("holders", &state.holders.len())
            .finish()
    }
}

impl Default for LeaseTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl LeaseTracker {
    /// Creates a tracker with no observer.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(level = tracing::Level::TRACE)
    )]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(TrackerInner {
                state: Mutex::new(TrackerState {
                    holders: HashMap::new(),
                }),
                observer: None,
            }),
        }
    }

    /// Creates a tracker that notifies `observer` on 0→1 and 1→0 transitions.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(observer), level = tracing::Level::TRACE)
    )]
    pub fn with_observer(observer: Arc<dyn LeaseObserver>) -> Self {
        Self {
            inner: Arc::new(TrackerInner {
                state: Mutex::new(TrackerState {
                    holders: HashMap::new(),
                }),
                observer: Some(observer),
            }),
        }
    }

    /// Acquires a named lease. Drop the returned guard to release it.
    #[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name),
            fields(
                name = tracing::field::Empty,
                lease_id = tracing::field::Empty,
                holders = tracing::field::Empty,
                first_holder = tracing::field::Empty,
            ),
            level = tracing::Level::TRACE,
        )
    )]
    pub fn acquire(&self, name: impl Into<LeaseName>) -> Lease {
        let name = name.into();
        let id = LeaseId::next();
        #[cfg(feature = "tracing")]
        {
            tracing::Span::current().record("name", tracing::field::display(&name));
            tracing::Span::current().record("lease_id", tracing::field::display(&id));
        }

        let became_first = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let was_empty = state.holders.is_empty();
            state.holders.insert(id, name.clone());
            let holders = state.holders.len();
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("holders", holders);
            tracing::debug!(
                lease_id = %id,
                name = %name,
                holders,
                first_holder = was_empty,
                "lease acquired"
            );
            was_empty
        };

        #[cfg(feature = "tracing")]
        tracing::Span::current().record("first_holder", became_first);

        let lease = Lease {
            tracker: self.clone(),
            id,
            name,
            active: true,
        };

        if became_first && let Some(observer) = &self.inner.observer {
            tracing::debug!(name = %lease.name(), "notifying observer of first holder");
            observer.on_first_acquire(lease.name());
        }

        lease
    }

    /// Runs `f` while holding a named lease.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, name, f),
            fields(name = tracing::field::Empty),
            level = tracing::Level::TRACE,
        )
    )]
    pub fn with<R>(&self, name: impl Into<LeaseName>, f: impl FnOnce() -> R) -> R {
        let name = name.into();
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("name", tracing::field::display(&name));
        let _lease = self.acquire(name);
        f()
    }

    /// Returns the number of active holders.
    pub fn len(&self) -> usize {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .holders
            .len()
    }

    /// Returns whether there are no active holders.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the names of all active holders.
    pub fn holders(&self) -> Vec<LeaseName> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .holders
            .values()
            .cloned()
            .collect()
    }

    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self),
            fields(
                name = %name,
                lease_id = %id,
                holders = tracing::field::Empty,
                last_holder = tracing::field::Empty,
            ),
            level = tracing::Level::TRACE,
        )
    )]
    fn release(&self, id: LeaseId, name: &LeaseName) {
        let became_empty = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.holders.remove(&id);
            let holders = state.holders.len();
            let empty = state.holders.is_empty();
            #[cfg(feature = "tracing")]
            {
                tracing::Span::current().record("holders", holders);
                tracing::Span::current().record("last_holder", empty);
            }
            tracing::debug!(
                lease_id = %id,
                name = %name,
                holders,
                last_holder = empty,
                "lease released"
            );
            empty
        };

        if became_empty && let Some(observer) = &self.inner.observer {
            tracing::debug!(name = %name, "notifying observer of last holder release");
            observer.on_last_release(name);
        }
    }
}

/// RAII guard for one named lease holder.
#[must_use = "lease is released immediately if unused; bind it (e.g. `let _lease = …`)"]
pub struct Lease {
    tracker: LeaseTracker,
    id: LeaseId,
    name: LeaseName,
    active: bool,
}

impl Lease {
    /// Returns this lease's id.
    pub fn id(&self) -> LeaseId {
        self.id
    }

    /// Returns this lease's name.
    pub fn name(&self) -> &LeaseName {
        &self.name
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.tracker.release(self.id, &self.name);
        }
    }
}

impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lease")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}

/// Runs `body` while holding a named lease on `tracker`.
///
/// Thin wrapper around [`LeaseTracker::with`]: expands to
/// `$tracker.with($name, || $body)`. The lease is acquired before `body`
/// runs and released when `body` returns (including early `return` / panic).
///
/// Prefer the [`lease`] attribute when the lease should
/// cover an entire function. Use this macro for an ad-hoc block.
///
/// # Examples
///
/// ```
/// use cadmus_core::lease::LeaseTracker;
/// use cadmus_core::with_lease;
///
/// let tracker = LeaseTracker::new();
/// let n = with_lease!(&tracker, "block", {
///     assert_eq!(tracker.len(), 1);
///     7
/// });
/// assert_eq!(n, 7);
/// assert!(tracker.is_empty());
/// ```
#[macro_export]
macro_rules! with_lease {
    ($tracker:expr, $name:expr, $body:block) => {{ $tracker.with($name, || $body) }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadmus_macros::lease;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    struct CountingObserver {
        first: AtomicUsize,
        last: AtomicUsize,
    }

    impl CountingObserver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                first: AtomicUsize::new(0),
                last: AtomicUsize::new(0),
            })
        }
    }

    impl LeaseObserver for CountingObserver {
        fn on_first_acquire(&self, _name: &LeaseName) {
            self.first.fetch_add(1, Ordering::SeqCst);
        }

        fn on_last_release(&self, _name: &LeaseName) {
            self.last.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn acquire_and_drop_updates_len() {
        let tracker = LeaseTracker::new();
        assert!(tracker.is_empty());
        let lease = tracker.acquire("a");
        assert_eq!(tracker.len(), 1);
        drop(lease);
        assert!(tracker.is_empty());
    }

    #[test]
    fn two_named_holders() {
        let tracker = LeaseTracker::new();
        let a = tracker.acquire("alpha");
        let b = tracker.acquire("beta");
        assert_eq!(tracker.len(), 2);
        let mut names: Vec<_> = tracker
            .holders()
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
        drop(a);
        assert_eq!(tracker.len(), 1);
        assert_eq!(tracker.holders()[0].as_str(), "beta");
        drop(b);
        assert!(tracker.is_empty());
    }

    #[test]
    fn observer_fires_on_empty_transitions() {
        let observer = CountingObserver::new();
        let tracker = LeaseTracker::with_observer(observer.clone());
        let a = tracker.acquire("a");
        assert_eq!(observer.first.load(Ordering::SeqCst), 1);
        assert_eq!(observer.last.load(Ordering::SeqCst), 0);
        let b = tracker.acquire("b");
        assert_eq!(observer.first.load(Ordering::SeqCst), 1);
        drop(a);
        assert_eq!(observer.last.load(Ordering::SeqCst), 0);
        drop(b);
        assert_eq!(observer.last.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn with_releases_after_success() {
        let tracker = LeaseTracker::new();
        let value = tracker.with("scoped", || 42);
        assert_eq!(value, 42);
        assert!(tracker.is_empty());
    }

    #[test]
    fn with_releases_after_early_return() {
        let tracker = LeaseTracker::new();
        let result: Result<(), &'static str> = tracker.with("scoped", || {
            return Err("early");
            #[allow(unreachable_code)]
            Ok(())
        });
        assert_eq!(result, Err("early"));
        assert!(tracker.is_empty());
    }

    #[test]
    fn with_releases_after_panic() {
        let tracker = LeaseTracker::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            tracker.with("scoped", || panic!("boom"));
        }));
        assert!(result.is_err());
        assert!(tracker.is_empty());
    }

    #[test]
    fn with_lease_macro_holds_and_releases() {
        let tracker = LeaseTracker::new();
        let value = with_lease!(&tracker, "macro", {
            assert_eq!(tracker.len(), 1);
            7
        });
        assert_eq!(value, 7);
        assert!(tracker.is_empty());
    }

    #[test]
    fn lease_attribute_holds_for_function() {
        #[lease(tracker, "attr")]
        fn work(tracker: &LeaseTracker) -> i32 {
            assert_eq!(tracker.len(), 1);
            9
        }

        let tracker = LeaseTracker::new();
        assert_eq!(work(&tracker), 9);
        assert!(tracker.is_empty());
    }

    #[test]
    fn lease_attribute_releases_on_early_return() {
        #[lease(tracker, "attr")]
        fn work(tracker: &LeaseTracker) -> Result<(), &'static str> {
            assert_eq!(tracker.len(), 1);
            Err("stop")
        }

        let tracker = LeaseTracker::new();
        assert_eq!(work(&tracker), Err("stop"));
        assert!(tracker.is_empty());
    }

    #[test]
    fn lease_attribute_on_field_path() {
        struct Host {
            tracker: LeaseTracker,
        }

        impl Host {
            #[lease(self.tracker, "field")]
            fn work(&self) -> usize {
                self.tracker.len()
            }
        }

        let host = Host {
            tracker: LeaseTracker::new(),
        };
        assert_eq!(host.work(), 1);
        assert!(host.tracker.is_empty());
    }

    struct FallibleTracker {
        inner: LeaseTracker,
        fail: bool,
    }

    impl FallibleTracker {
        fn acquire(&self, name: impl Into<LeaseName>) -> Result<Lease, &'static str> {
            if self.fail {
                Err("denied")
            } else {
                Ok(self.inner.acquire(name))
            }
        }
    }

    #[test]
    fn lease_attribute_or_return_holds_on_ok() {
        struct Host {
            tracker: FallibleTracker,
        }

        impl Host {
            #[lease(self.tracker, "or-return", or_return)]
            fn work(&self) {
                assert_eq!(self.tracker.inner.len(), 1);
            }
        }

        let host = Host {
            tracker: FallibleTracker {
                inner: LeaseTracker::new(),
                fail: false,
            },
        };
        host.work();
        assert!(host.tracker.inner.is_empty());
    }

    #[test]
    fn lease_attribute_or_return_exits_on_err() {
        struct Host {
            tracker: FallibleTracker,
            ran: std::cell::Cell<bool>,
        }

        impl Host {
            #[lease(
                self.tracker,
                "or-return",
                or_return("failed to acquire WiFi lease for time sync")
            )]
            fn work(&self) {
                self.ran.set(true);
            }
        }

        let host = Host {
            tracker: FallibleTracker {
                inner: LeaseTracker::new(),
                fail: true,
            },
            ran: std::cell::Cell::new(false),
        };
        host.work();
        assert!(!host.ran.get());
        assert!(host.tracker.inner.is_empty());
    }

    #[test]
    fn concurrent_acquire_release() {
        let tracker = LeaseTracker::new();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let tracker = tracker.clone();
                thread::spawn(move || {
                    for _ in 0..50 {
                        let name = format!("t{i}");
                        let _lease = tracker.acquire(name);
                        assert!(!tracker.is_empty());
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("thread panicked");
        }
        assert!(tracker.is_empty());
    }
}
