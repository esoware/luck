//! # `luck_allocator`
//!
//! The bump arena luck's syntax trees live in. An [`Allocator`] hands out memory that is freed all
//! at once, when it's [`reset`](Allocator::reset) or dropped. A [`Box`] holds one value in it and a
//! [`List`] a run of values, and neither is ever dropped. An [`AllocatorPool`] recycles allocators
//! across threads.
//!
//! Everything but [`List`] is ported from `oxc_allocator` 0.159.0, at oxc commit
//! `b6ff5069de518a3d8dfd75c931da12eb9746af15`, and kept close enough to it that a diff against
//! that commit shows upstream's later fixes. Its arena derives from `bumpalo`. The notices for both
//! are in `LICENSE`.

mod allocator;
mod boxed;
mod list;
mod pool;

// Only expose `arena` module for doc tests
#[cfg(not(feature = "testing"))]
mod arena;
#[cfg(feature = "testing")]
pub mod arena;

pub use allocator::Allocator;
pub use boxed::Box;
pub use list::{List, ListIntoIter};
pub use pool::{AllocatorGuard, AllocatorPool};
