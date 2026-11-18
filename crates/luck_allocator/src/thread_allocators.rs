use thread_local::ThreadLocal;

use crate::Allocator;

/// An [`Allocator`] for each thread that asks for one, all of which live as long as this does.
///
/// Work spread over a thread pool builds in the allocator of the thread that runs it, and what it
/// builds outlives the work. `&Allocator` isn't `Send`, so the allocator a thread gets never leaves
/// it, and no two threads ever build in one at once.
#[derive(Debug, Default)]
pub struct ThreadAllocators {
    allocators: ThreadLocal<Allocator>,
}

impl ThreadAllocators {
    /// Creates a set that makes each thread's allocator on its first [`get`](Self::get).
    pub const fn new() -> Self {
        Self { allocators: ThreadLocal::new() }
    }

    /// The calling thread's allocator.
    pub fn get(&self) -> &Allocator {
        self.allocators.get_or_default()
    }

    /// Resets every thread's allocator, each keeping its memory for what's built next.
    pub fn reset(&mut self) {
        for allocator in &mut self.allocators {
            allocator.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::thread;

    use super::*;

    #[test]
    fn threads_build_in_their_own_allocators_and_the_builds_outlive_them() {
        let mut allocators = ThreadAllocators::new();
        // Both threads stay alive until both have built, since a thread that exits gives its id,
        // and with it its allocator, to the next thread that starts.
        let both_built = Barrier::new(2);
        let built: Vec<&str> = thread::scope(|scope| {
            let workers = ["a", "b"].map(|text| {
                let (allocators, both_built) = (&allocators, &both_built);
                scope.spawn(move || {
                    let built = allocators.get().alloc_str(text);
                    both_built.wait();
                    built
                })
            });
            workers.map(|worker| worker.join().unwrap()).to_vec()
        });
        assert_eq!(built, ["a", "b"]);
        assert_eq!(allocators.allocators.iter_mut().count(), 2);
        allocators.reset();
        assert!(allocators.allocators.iter_mut().all(|allocator| allocator.used_bytes() == 0));
    }
}
