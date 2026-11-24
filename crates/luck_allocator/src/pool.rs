use std::{iter, mem, ops::Deref, sync::PoisonError};

use crate::Allocator;

/// Allocators for a pool of threads, each lent to one thread at a time and reset when it comes
/// back, so a run keeps about one allocator per thread however much work it does.
#[derive(Debug)]
pub struct AllocatorPool {
    #[expect(
        clippy::disallowed_types,
        reason = "parking_lot isn't a dependency here, and nothing that can panic holds the lock"
    )]
    allocators: std::sync::Mutex<Vec<Allocator>>,
}

impl AllocatorPool {
    /// A pool that starts with an allocator for each of `thread_count` threads.
    pub fn new(thread_count: usize) -> Self {
        let allocators: Vec<Allocator> =
            iter::repeat_with(Allocator::new).take(thread_count).collect();
        Self { allocators: allocators.into() }
    }

    /// An allocator from the pool, or a new one when the pool has none to spare.
    pub fn get(&self) -> AllocatorGuard<'_> {
        let spare = self.allocators.lock().unwrap_or_else(PoisonError::into_inner).pop();
        let allocator = spare.unwrap_or_default();
        AllocatorGuard { allocator, pool: self }
    }
}

/// An allocator lent from an [`AllocatorPool`], which goes back to the pool reset when the guard
/// drops.
#[derive(Debug)]
pub struct AllocatorGuard<'pool> {
    allocator: Allocator,
    pool: &'pool AllocatorPool,
}

impl Deref for AllocatorGuard<'_> {
    type Target = Allocator;

    fn deref(&self) -> &Allocator {
        &self.allocator
    }
}

impl Drop for AllocatorGuard<'_> {
    fn drop(&mut self) {
        // A new allocator owns no memory, so the one left in its place has nothing to free.
        let mut allocator = mem::take(&mut self.allocator);
        allocator.reset();
        self.pool.allocators.lock().unwrap_or_else(PoisonError::into_inner).push(allocator);
    }
}
