use luck_allocator::{AllocationCounts, Allocator};

use crate::heap::{HEAP, HeapCounters};

/// One stage's activity on one file.
#[derive(Debug)]
pub(crate) struct StageStats {
    pub heap: HeapCounters,
    pub arena: AllocationCounts,
    /// The arena isn't reset between stages, so this includes what earlier
    /// stages left in it, such as the tree the parser built.
    pub arena_used_bytes: usize,
    /// How far live heap memory rose above where it started during the stage,
    /// which catches memory the stage needs even when it frees it again.
    pub sys_peak_growth_bytes: usize,
}

impl StageStats {
    /// Every metric a snapshot holds of the stage, in order. A stage's
    /// snapshot heads them with [`Metric::file_size`], and the per-rule
    /// report leaves that out.
    pub(crate) fn metrics(&self) -> [Metric; 8] {
        let Self { heap, arena, arena_used_bytes, sys_peak_growth_bytes } = *self;
        [
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
                sys_peak_growth_bytes,
                relative_tolerance(sys_peak_growth_bytes),
            ),
            Metric::count("arena allocs", arena.allocations),
            Metric::count("arena reallocs", arena.reallocations),
            Metric::bytes("arena size", arena_used_bytes, ARENA_SIZE_TOLERANCE_BYTES),
        ]
    }
}

/// Runs `stage` and records what it allocated.
///
/// Chunk operations need no correction here, because the global allocator
/// already left them out, and every remaining counter grows with element
/// counts, which are the same on every platform.
pub(crate) fn record_stats_in<R>(
    allocator: &Allocator,
    stage: impl FnOnce() -> R,
) -> (R, StageStats) {
    let (heap_before, arena_before) = (HEAP.read(), allocator.allocation_counts());
    let live_bytes_before = HEAP.live_bytes();
    HEAP.reset_peak();
    let result = stage();
    let (heap, arena) = (HEAP.read(), allocator.allocation_counts());
    let stats = StageStats {
        heap: HeapCounters {
            allocs: heap.allocs - heap_before.allocs,
            reallocs: heap.reallocs - heap_before.reallocs,
            deallocs: heap.deallocs - heap_before.deallocs,
            alloc_bytes: heap.alloc_bytes - heap_before.alloc_bytes,
        },
        arena: AllocationCounts {
            allocations: arena.allocations - arena_before.allocations,
            reallocations: arena.reallocations - arena_before.reallocations,
        },
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
    (value / 100).max(4096)
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
    /// The measured file's size, which heads its entry in a stage's snapshot.
    pub(crate) const fn file_size(bytes: usize) -> Self {
        Self::bytes("file size", bytes, 0)
    }

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
