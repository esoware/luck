//! # `luck_allocator`
//!
//! The bump arena luck's syntax trees live in. An [`Allocator`] hands out memory that is freed all
//! at once, when it's [`reset`](Allocator::reset) or dropped. A [`Box`] holds one value in it and a
//! [`List`] a run of values, and neither is ever dropped. A [`ThinStr`] holds a `&str` in 12 bytes,
//! and a [`ThinBytes`] a `&[u8]`. An [`AllocatorPool`] recycles allocators across threads, and
//! [`ThreadAllocators`] gives each thread one that outlives the thread's work.
//!
//! The `track_allocations` feature counts each allocator's allocations, and marks the calls it
//! makes to the global allocator for its chunks, so a global allocator that counts calls can leave
//! them out.
//!
//! The arena began as `oxc_allocator`'s, which derives from `bumpalo`, and [`Box`] as `jsparagus`'s.
//! Their notices are in `LICENSE`.

mod allocator;
mod boxed;
mod list;
mod pool;
mod thin_bytes;
mod thin_str;
mod thread_allocators;
#[cfg(feature = "track_allocations")]
mod tracking;

pub use allocator::Allocator;
pub use boxed::Box;
pub use list::{List, ListIntoIter};
pub use pool::{AllocatorGuard, AllocatorPool};
pub use thin_bytes::ThinBytes;
pub use thin_str::ThinStr;
pub use thread_allocators::ThreadAllocators;
#[cfg(feature = "track_allocations")]
pub use tracking::{AllocationCounts, take_pending_chunk_operation};
