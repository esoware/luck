# luck_allocator

A bump allocation arena for syntax trees.

Allocating from an `Allocator` bumps a pointer within a chunk of memory, and a
new chunk comes from the global allocator when the current one fills up.
Nothing allocated in the arena is ever dropped. Instead, the whole arena is
freed at once when it's reset or dropped, which suits a syntax tree that lives
exactly as long as the file it came from.

`reset` keeps the arena's memory, so one arena can be reused for file after
file. The arena started out as `oxc_allocator`'s, which derives from `bumpalo`.

## Usage

```rust
use luck_allocator::{Allocator, AllocatorPool};

let mut allocator = Allocator::new();
let name = allocator.alloc_str("greeting");
assert_eq!(name, "greeting");
allocator.reset();

let pool = AllocatorPool::new(4);
let arena = pool.get();
let value = arena.alloc(42);
assert_eq!(*value, 42);
```

## API

* `Allocator`, the arena, with `alloc`, `alloc_str`, `alloc_slice_copy`, and
  `reset`.
* `Box`, a single value in the arena.
* `List`, a sequence of values in the arena, and `ListIntoIter`, which iterates
  one by value.
* `ThinStr` and `ThinBytes`, a `&str` and a `&[u8]` stored in 12 bytes.
* `TextBytes`, bytes checked as UTF-8 once, which hands out a `ThinStr` of any
  range.
* `AllocatorPool`, which lends arenas to threads. Each comes in an
  `AllocatorGuard`, which resets the arena and returns it when dropped.
* `ThreadAllocators`, which gives every thread its own arena.

## Cargo features

* `track_allocations` - Counts the allocations each arena makes, and marks the
  chunks it requests from the global allocator. This is for luck's allocation
  snapshots and isn't meant for other use.
