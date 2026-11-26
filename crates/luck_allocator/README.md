# luck_allocator

A bump arena allocator for luck's syntax trees, with arena-backed `Box`, `List`, and compact string types.

## Overview

Every luck syntax tree lives in an `Allocator`. Nothing in it is ever dropped, so don't put a type that needs `Drop` in it.

`reset` frees everything at once and keeps the memory for the next file.

`AllocatorPool` and `ThreadAllocators` hand out arenas to worker threads.

Based on `oxc_allocator`, which is based on `bumpalo`.

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

- `Allocator` is the arena, with `alloc`, `alloc_str`, `alloc_slice_copy`, and `reset`.
- `Box` holds one value in the arena.
- `List` holds a sequence of values in the arena, and `ListIntoIter` iterates it by value.
- `ThinStr` is a `&str` in 12 bytes, and `ThinBytes` is the same for `&[u8]`.
- `TextBytes` checks bytes for UTF-8 once, then hands out `ThinStr`s of its ranges.
- `AllocatorPool` shares arenas between threads. `get` returns an `AllocatorGuard`, which resets the arena and gives it back on drop.
- `ThreadAllocators` gives each thread its own arena, which outlives the thread's work.

## Cargo features

- `track_allocations` counts each arena's allocations, for `luck_allocs`. Internal use only.
