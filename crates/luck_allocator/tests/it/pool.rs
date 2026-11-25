use std::thread;

use luck_allocator::{AllocatorGuard, AllocatorPool};

use crate::common::{AutoTraits, Fallback as _};

#[test]
fn a_returned_allocator_comes_back_reset_with_its_memory() {
    let pool = AllocatorPool::new(1);
    let capacity = {
        let allocator = pool.get();
        allocator.alloc_str("kept");
        assert!(allocator.used_bytes() > 0);
        allocator.capacity()
    };
    let allocator = pool.get();
    assert_eq!(allocator.used_bytes(), 0);
    assert_eq!(allocator.capacity(), capacity);
}

#[test]
fn guards_held_at_once_get_different_allocators() {
    let pool = AllocatorPool::new(1);
    let first = pool.get();
    let second = pool.get();
    first.alloc(1_u8);
    assert_eq!(second.used_bytes(), 0);
}

#[test]
fn serves_several_threads() {
    let pool = AllocatorPool::new(4);
    thread::scope(|scope| {
        for worker in 0..4 {
            let pool = &pool;
            scope.spawn(move || {
                for file in 0..3 {
                    let allocator = pool.get();
                    let text = allocator.alloc_str(&format!("worker {worker} file {file}"));
                    assert_eq!(text, format!("worker {worker} file {file}"));
                }
            });
        }
    });
}

#[test]
fn auto_traits_keep_each_allocator_on_one_thread() {
    const {
        assert!(AutoTraits::<AllocatorPool>::IS_SEND);
        assert!(AutoTraits::<AllocatorPool>::IS_SYNC);
        assert!(AutoTraits::<AllocatorGuard<'_>>::IS_SEND);
        assert!(!AutoTraits::<AllocatorGuard<'_>>::IS_SYNC);
    }
}
