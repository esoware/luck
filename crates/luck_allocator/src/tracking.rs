//! Only `luck_allocs` turns tracking on, since counting costs every allocation a little.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::Allocator;

/// Whether the global allocator's next call allocates or frees an arena's chunk.
///
/// A global allocator that counts calls leaves chunks out, since their sizes are quantized and
/// whether an arena needs one more depends on byte totals that vary with each target's type
/// layout, which would make the counts differ between platforms.
///
/// It's a mark rather than a count: an arena sets it right before the call, and the global
/// allocator takes it while the call is on the stack, with no allocation between. Another thread
/// allocating at that moment could take it instead, but `luck_allocs` measures on one thread.
static IS_CHUNK_OPERATION_PENDING: AtomicBool = AtomicBool::new(false);

/// Marks the global allocator's next call as an arena chunk's.
pub(crate) fn start_chunk_operation() {
    IS_CHUNK_OPERATION_PENDING.store(true, Ordering::Relaxed);
}

/// Whether the global allocator's call on the stack allocates or frees an arena's chunk, which a
/// global allocator that counts calls leaves out. It takes the mark, so a global allocator calls
/// it once per call, whether or not the call succeeds.
pub fn take_pending_chunk_operation() -> bool {
    IS_CHUNK_OPERATION_PENDING.swap(false, Ordering::Relaxed)
}

/// What an [`Allocator`] has counted since it was made or last reset. Each count saturates at
/// `usize::MAX`, past which the exact number doesn't matter.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllocationCounts {
    /// Allocations, apart from the ones a `List` grew into.
    pub allocations: usize,
    /// Times a full `List` grew, by moving to a bigger allocation.
    pub reallocations: usize,
}

impl AllocationCounts {
    pub(crate) const fn with_allocation(self) -> Self {
        Self { allocations: self.allocations.saturating_add(1), ..self }
    }

    /// The allocation just counted recounted as a reallocation, since a `List` grew into it.
    pub(crate) const fn with_reallocation(self) -> Self {
        Self {
            // A saturated count may stand for more than it shows, so it stays.
            allocations: if self.allocations == usize::MAX {
                usize::MAX
            } else {
                self.allocations - 1
            },
            reallocations: self.reallocations.saturating_add(1),
        }
    }
}

impl Allocator {
    pub fn allocation_counts(&self) -> AllocationCounts {
        self.counts.get()
    }
}
