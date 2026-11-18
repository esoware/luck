# luck_allocator

The bump arena that luck's syntax trees live in.

## Overview

An `Allocator` hands out memory from chunks it grows as needed and frees it all at once, when it is reset or dropped. Nothing in it is dropped on its own, so a whole tree goes in one step, and `reset` keeps the largest chunk for the next file. An arena serves one thread at a time: it is `Send` but not `Sync`, so threads each take their own, from an `AllocatorPool` or a slice the caller holds.

Everything but `List` and `ThinStr` is ported from `oxc_allocator`, whose notices are in `LICENSE`.

## Public surface

- `Allocator`: the arena. `alloc` and `alloc_str` copy a value or a string into it, and `reset` empties it.
- `Box`: one value in the arena, never dropped. `Box::new_in` refuses to compile for a type that needs `Drop`.
- `List`: a run of values in the arena, built from an iterator or array of known length, or grown with `push`. It derefs to a slice.
- `ThinStr`: a `&str` with a 32-bit length, so a node can pack a small field beside it.
- `AllocatorPool`: arenas for a pool of threads. `get` returns an `AllocatorGuard`, which resets its arena and hands it back when it drops.

```rust
use luck_allocator::{Allocator, Box, List};

let mut allocator = Allocator::new();
let boxed = Box::new_in(5, &allocator);
let list = List::from_array_in([1, 2, 3], &allocator);
assert_eq!((*boxed, list.len()), (5, 3));
allocator.reset();
```
