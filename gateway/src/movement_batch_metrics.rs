//! Lock-free observations at the movement batch submission boundary.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MovementBatchSnapshot {
    pub entries: u64,
    pub calls: u64,
    pub failed_entries: u64,
    pub failed_calls: u64,
    pub sizes: [u64; 5],
}

impl MovementBatchSnapshot {
    pub fn log_line(self) -> String {
        format!(
            "MOVEBATCH entries={} calls={} failed_entries={} failed_calls={} size_1={} size_2_32={} size_33_64={} size_65_127={} size_128={}",
            self.entries,
            self.calls,
            self.failed_entries,
            self.failed_calls,
            self.sizes[0],
            self.sizes[1],
            self.sizes[2],
            self.sizes[3],
            self.sizes[4]
        )
    }
}

pub(crate) struct MovementBatchMetrics {
    entries: AtomicU64,
    calls: AtomicU64,
    failed_entries: AtomicU64,
    failed_calls: AtomicU64,
    sizes: [AtomicU64; 5],
}

impl MovementBatchMetrics {
    pub const fn new() -> Self {
        Self {
            entries: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            failed_entries: AtomicU64::new(0),
            failed_calls: AtomicU64::new(0),
            sizes: [const { AtomicU64::new(0) }; 5],
        }
    }

    /// Record one attempted reducer call. Entries in a failed call are reported as dropped,
    /// separately from successful submissions.
    pub fn observe(&self, entries: usize, succeeded: bool) {
        let entries = entries as u64;
        self.entries.fetch_add(entries, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        let bucket = match entries {
            1 => 0,
            2..=32 => 1,
            33..=64 => 2,
            65..=127 => 3,
            _ => 4,
        };
        self.sizes[bucket].fetch_add(1, Ordering::Relaxed);
        if !succeeded {
            self.failed_entries.fetch_add(entries, Ordering::Relaxed);
            self.failed_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> MovementBatchSnapshot {
        MovementBatchSnapshot {
            entries: self.entries.load(Ordering::Relaxed),
            calls: self.calls.load(Ordering::Relaxed),
            failed_entries: self.failed_entries.load(Ordering::Relaxed),
            failed_calls: self.failed_calls.load(Ordering::Relaxed),
            sizes: std::array::from_fn(|i| self.sizes[i].load(Ordering::Relaxed)),
        }
    }
}

pub(crate) static MOVEMENT_BATCH_METRICS: MovementBatchMetrics = MovementBatchMetrics::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_interval_has_zero_calls_and_no_invented_ratio_operand() {
        let metrics = MovementBatchMetrics::new();
        assert_eq!(metrics.snapshot(), Default::default());
    }

    #[test]
    fn normal_capped_and_failed_calls_are_distinct() {
        let metrics = MovementBatchMetrics::new();
        metrics.observe(24, true);
        metrics.observe(128, true);
        metrics.observe(7, false);
        assert_eq!(
            metrics.snapshot(),
            MovementBatchSnapshot {
                entries: 159,
                calls: 3,
                failed_entries: 7,
                failed_calls: 1,
                sizes: [0, 2, 0, 0, 1],
            }
        );
    }
}
