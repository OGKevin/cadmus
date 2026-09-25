//! Choose in-memory vs streaming buffering from available RAM.
//!
//! Callers estimate how many bytes they need; we compare that to
//! [`sysinfo`]'s available memory (Linux: `MemAvailable`) minus a safety
//! margin reserved for the rest of the app.

use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// Fraction of available RAM kept free for the rest of the process.
const AVAILABLE_MARGIN: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferPath {
    InMemory,
    Stream,
}

/// Returns whether `needed_bytes` fits in available RAM after the safety margin.
#[must_use]
pub fn choose_buffer_path(needed_bytes: u64) -> BufferPath {
    let mut system = System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    let available = system.available_memory();
    let budget = (available as f64 * (1.0 - AVAILABLE_MARGIN)) as u64;
    let path = if needed_bytes <= budget {
        BufferPath::InMemory
    } else {
        BufferPath::Stream
    };
    tracing::debug!(
        needed_bytes,
        available_bytes = available,
        budget_bytes = budget,
        ?path,
        "chose buffer path from available memory"
    );
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_bytes_fits_in_memory() {
        assert_eq!(choose_buffer_path(0), BufferPath::InMemory);
    }

    #[test]
    fn absurd_size_streams() {
        assert_eq!(choose_buffer_path(u64::MAX / 2), BufferPath::Stream);
    }
}
