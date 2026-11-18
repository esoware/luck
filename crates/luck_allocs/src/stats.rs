use luck_allocator::Allocator;

use crate::heap::{HEAP, HeapCounters};

/// One stage's activity on one file.
#[derive(Debug)]
pub(crate) struct StageStats {
    pub counters: Counters,
    /// The arena isn't reset between stages, so this includes what earlier
    /// stages left in it, such as the tree the parser built.
    pub arena_used_bytes: usize,
    /// How far live heap memory rose above where it started during the stage,
    /// which catches memory the stage needs even when it frees it again.
    pub sys_peak_growth_bytes: usize,
}

impl StageStats {
    /// Every metric a snapshot holds, in order. This is the one place that
    /// decides what gets snapshotted.
    pub(crate) fn metrics(&self, file_bytes: usize) -> Vec<Metric> {
        let Counters { heap, arena } = &self.counters;
        vec![
            Metric::bytes("file size", file_bytes, 0),
            Metric::count("sys allocs", heap.allocs),
            Metric::count("sys reallocs", heap.reallocs),
            Metric::count("sys deallocs", heap.deallocs),
            Metric::bytes(
                "sys alloc bytes",
                heap.alloc_bytes,
                relative_tolerance(heap.alloc_bytes),
            ),
            Metric::bytes(
                "sys peak growth",
                self.sys_peak_growth_bytes,
                relative_tolerance(self.sys_peak_growth_bytes),
            ),
            Metric::count("arena allocs", arena.allocs),
            Metric::count("arena reallocs", arena.reallocs),
            Metric::bytes("arena size", self.arena_used_bytes, ARENA_SIZE_TOLERANCE_BYTES),
        ]
    }
}

/// The heap's and the arena's counters, as a reading or as the difference
/// between two.
#[derive(Debug)]
pub(crate) struct Counters {
    pub heap: HeapCounters,
    pub arena: ArenaCounters,
}

/// A reallocation is a `List` growing, which moves it to a bigger arena
/// allocation.
#[derive(Debug)]
pub(crate) struct ArenaCounters {
    pub allocs: usize,
    pub reallocs: usize,
}

impl Counters {
    fn record(allocator: &Allocator) -> Self {
        let (allocs, reallocs) = allocator.get_allocation_stats();
        Self { heap: HEAP.read(), arena: ArenaCounters { allocs, reallocs } }
    }

    /// Chunk operations need no correction here, because the global
    /// allocator already left them out, and every remaining counter grows
    /// with element counts, which are the same on every platform.
    const fn since(&self, before: &Self) -> Self {
        Self {
            heap: HeapCounters {
                allocs: self.heap.allocs.saturating_sub(before.heap.allocs),
                reallocs: self.heap.reallocs.saturating_sub(before.heap.reallocs),
                deallocs: self.heap.deallocs.saturating_sub(before.heap.deallocs),
                alloc_bytes: self.heap.alloc_bytes.saturating_sub(before.heap.alloc_bytes),
            },
            arena: ArenaCounters {
                allocs: self.arena.allocs.saturating_sub(before.arena.allocs),
                reallocs: self.arena.reallocs.saturating_sub(before.arena.reallocs),
            },
        }
    }
}

/// Runs `stage` and records what it allocated.
pub(crate) fn record_stats_in<R>(
    allocator: &Allocator,
    stage: impl FnOnce() -> R,
) -> (R, StageStats) {
    let before = Counters::record(allocator);
    let live_bytes_before = HEAP.live_bytes();
    HEAP.reset_peak();
    let result = stage();
    let stats = StageStats {
        counters: Counters::record(allocator).since(&before),
        arena_used_bytes: allocator.used_bytes(),
        sys_peak_growth_bytes: HEAP.peak_bytes().saturating_sub(live_bytes_before),
    };
    (result, stats)
}

/// Arena content drifts by at most 128 bytes per file between
/// architectures, so this is an 8x margin that still catches any real
/// change, which moves arena sizes by orders of magnitude more.
const ARENA_SIZE_TOLERANCE_BYTES: usize = 1024;

/// 1% of `value`, with a floor for small values.
///
/// Every heap hash table a stage allocates drifts by a few bytes between
/// architectures, so the drift scales with the work and no flat tolerance
/// fits both small and large values.
fn relative_tolerance(value: usize) -> usize {
    value.div_euclid(100).max(4096)
}

#[derive(Debug)]
pub(crate) struct Metric {
    /// The key it's written under, and its committed value read back by.
    pub label: &'static str,
    pub value: usize,
    pub kind: MetricKind,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MetricKind {
    /// The same on every platform, so it's snapshotted exactly.
    Count,
    /// Rendered with a human-readable size beside it.
    ///
    /// Some type layouts differ between targets, such as hashbrown's control
    /// groups, which are 16 bytes on `x86_64` and 8 on aarch64, so a byte total can move a few
    /// bytes between platforms. Keeping the committed value when the measured
    /// one is within `tolerance` of it lets a snapshot written on one
    /// platform pass on the others, and a real regression is far larger.
    Bytes { tolerance: usize },
}

impl Metric {
    const fn count(label: &'static str, value: usize) -> Self {
        Self { label, value, kind: MetricKind::Count }
    }

    const fn bytes(label: &'static str, value: usize, tolerance: usize) -> Self {
        Self { label, value, kind: MetricKind::Bytes { tolerance } }
    }

    /// The value to write, given the committed one, if any.
    pub(crate) fn snapshot_value(&self, committed: Option<i64>) -> usize {
        let MetricKind::Bytes { tolerance } = self.kind else {
            return self.value;
        };
        let Some(committed_bytes) = committed.and_then(|value| usize::try_from(value).ok()) else {
            return self.value;
        };
        if self.value.abs_diff(committed_bytes) <= tolerance { committed_bytes } else { self.value }
    }
}
