use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering::SeqCst;

use luck_allocator::Allocator;
use mimalloc_safe::MiMalloc;

/// The global allocator: [`MiMalloc`], the CLI's production allocator, with
/// every call recorded in [`HEAP`].
#[derive(Debug)]
pub(crate) struct TrackedAllocator;

/// Saturates at `usize::MAX` instead of wrapping, which only a 32-bit target
/// could reach. `SeqCst` is stronger than needed, but the counts matter here,
/// not the speed.
#[derive(Debug)]
struct AtomicCounter(AtomicUsize);

impl AtomicCounter {
    const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    fn get(&self) -> usize {
        self.0.load(SeqCst)
    }

    /// Returns the new value.
    fn update(&self, change: impl Fn(usize) -> usize) -> usize {
        let mut current = self.0.load(SeqCst);
        loop {
            let changed = change(current);
            match self.0.compare_exchange_weak(current, changed, SeqCst, SeqCst) {
                Ok(_) => return changed,
                Err(actual) => current = actual,
            }
        }
    }

    fn increment(&self) {
        self.update(|count| count.saturating_add(1));
    }

    /// Returns the new value.
    fn add(&self, n: usize) -> usize {
        self.update(|count| count.saturating_add(n))
    }

    fn sub(&self, n: usize) {
        self.update(|count| count.saturating_sub(n));
    }

    fn update_max(&self, value: usize) {
        self.update(|count| count.max(value));
    }

    fn set(&self, value: usize) {
        self.0.store(value, SeqCst);
    }
}

/// The system allocator's live counters.
///
/// Arena chunk operations are left out of all of them: whether an arena needs
/// one more chunk depends on byte totals that vary with each target's type
/// layout, and a long-lived arena's chunks reflect its history rather than
/// the measured stage.
#[derive(Debug)]
pub(crate) struct HeapTracker {
    allocs: AtomicCounter,
    reallocs: AtomicCounter,
    deallocs: AtomicCounter,
    /// Allocation sizes plus realloc growth. Only ever grows.
    alloc_bytes: AtomicCounter,
    /// Never reset, because it gauges real live memory.
    live_bytes: AtomicCounter,
    /// The high-water mark of `live_bytes` since the last `reset_peak`.
    peak_bytes: AtomicCounter,
}

pub(crate) static HEAP: HeapTracker = HeapTracker {
    allocs: AtomicCounter::new(),
    reallocs: AtomicCounter::new(),
    deallocs: AtomicCounter::new(),
    alloc_bytes: AtomicCounter::new(),
    live_bytes: AtomicCounter::new(),
    peak_bytes: AtomicCounter::new(),
};

#[derive(Debug)]
pub(crate) struct HeapCounters {
    pub allocs: usize,
    pub reallocs: usize,
    pub deallocs: usize,
    pub alloc_bytes: usize,
}

impl HeapTracker {
    fn record_alloc(&self, size: usize) {
        self.allocs.increment();
        self.grow(size);
    }

    fn record_dealloc(&self, size: usize) {
        self.deallocs.increment();
        self.live_bytes.sub(size);
    }

    fn record_realloc(&self, old_size: usize, new_size: usize) {
        self.reallocs.increment();
        match new_size.checked_sub(old_size) {
            Some(growth) => self.grow(growth),
            None => self.live_bytes.sub(old_size.saturating_sub(new_size)),
        }
    }

    fn grow(&self, delta: usize) {
        self.alloc_bytes.add(delta);
        let live_bytes = self.live_bytes.add(delta);
        self.peak_bytes.update_max(live_bytes);
    }

    pub(crate) fn live_bytes(&self) -> usize {
        self.live_bytes.get()
    }

    pub(crate) fn peak_bytes(&self) -> usize {
        self.peak_bytes.get()
    }

    pub(crate) fn read(&self) -> HeapCounters {
        HeapCounters {
            allocs: self.allocs.get(),
            reallocs: self.reallocs.get(),
            deallocs: self.deallocs.get(),
            alloc_bytes: self.alloc_bytes.get(),
        }
    }

    /// Starts a new peak window from the current live size.
    pub(crate) fn reset_peak(&self) {
        self.peak_bytes.set(self.live_bytes.get());
    }

    /// `live_bytes` and `peak_bytes` gauge real memory, so they stay.
    pub(crate) fn reset(&self) {
        self.allocs.set(0);
        self.reallocs.set(0);
        self.deallocs.set(0);
        self.alloc_bytes.set(0);
    }
}

#[expect(unsafe_code, reason = "a global allocator can only be written with unsafe")]
// SAFETY: Every method delegates to `MiMalloc` with its caller's arguments
// unchanged, and only counts what it returns.
//
// Each method consumes the arena's chunk marker whether or not the call
// succeeds, so taking it comes first in every condition.
unsafe impl GlobalAlloc for TrackedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's `GlobalAlloc::alloc` contract is forwarded.
        let ptr = unsafe { MiMalloc.alloc(layout) };
        if !Allocator::take_pending_chunk_operation() && !ptr.is_null() {
            HEAP.record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !Allocator::take_pending_chunk_operation() {
            HEAP.record_dealloc(layout.size());
        }
        // SAFETY: `ptr` came from one of this allocator's methods, all of
        // which return `MiMalloc`'s memory, with `layout`.
        unsafe {
            MiMalloc.dealloc(ptr, layout);
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's `GlobalAlloc::alloc_zeroed` contract is
        // forwarded.
        let ptr = unsafe { MiMalloc.alloc_zeroed(layout) };
        if !Allocator::take_pending_chunk_operation() && !ptr.is_null() {
            HEAP.record_alloc(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: The caller's `GlobalAlloc::realloc` contract is forwarded,
        // and `ptr` is `MiMalloc`'s memory as in `dealloc`.
        let reallocated = unsafe { MiMalloc.realloc(ptr, layout, new_size) };
        if !Allocator::take_pending_chunk_operation() && !reallocated.is_null() {
            HEAP.record_realloc(layout.size(), new_size);
        }
        reallocated
    }
}
