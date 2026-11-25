# luck_allocator

The bump arena that luck's syntax trees live in.

## Overview

An `Allocator` hands out memory from chunks it grows as needed and frees it all at once, when it is reset or dropped. Nothing in it is dropped on its own, so freeing a whole tree takes one step, and `reset` keeps the largest chunk for the next file. An arena serves one thread at a time. It is `Send` but not `Sync`, so each thread takes its own, from an `AllocatorPool` or a `ThreadAllocators`.

An allocation bumps a cursor down from the end of the current chunk, and every allocation starts at a multiple of 8. A tree's node is aligned to 8 or 4 and sized in multiples of 8, so its allocation takes one subtraction and one bounds check, and a slice of bytes, which is rare beside the nodes, pays for the rule with up to 7 bytes of padding.

A `List` built from a scratch `Vec` with `split_off_in` takes one arena allocation of exactly its length and one copy, so a parser can gather the items of nested lists on one stack of `Vec`s and freeze each as it closes.

The arena began as `oxc_allocator`'s, which derives from `bumpalo`, and `Box` as `jsparagus`'s. Their notices are in `LICENSE`.

## Usage

- `Allocator`: the arena. `alloc`, `alloc_str`, and `alloc_slice_copy` copy a value, a string, or a slice into it, `alloc_concat_strs_array` joins strings into it, and `reset` empties it. `capacity` and `used_bytes` measure its chunks and what's allocated in them.
- `Box`: one value in the arena, never dropped. `Box::new_in` refuses to compile for a type that needs `Drop`.
- `List`: a run of values in the arena, built from an iterator or array of known length, or grown with `push`. It derefs to a slice.
- `ThinStr` and `ThinBytes`: a `&str` and a `&[u8]` with a 32-bit length, so a node can pack a small field beside one.
- `TextBytes`: bytes, such as a whole source, checked once for ASCII and then for UTF-8, so the text of a range that starts and ends with an ASCII byte, as a name's does, takes no check of its own in ASCII, only of those two bytes in UTF-8, and of every byte in anything else.
- `AllocatorPool`: arenas for a pool of threads. `get` returns an `AllocatorGuard`, which resets its arena and hands it back when it drops.
- `ThreadAllocators`: an arena for each thread that asks for one, which lives as long as the set does, so what a thread pool's work builds outlives the work. `get` returns the calling thread's, and `reset` empties them all.

The `track_allocations` feature, which only `luck_allocs` turns on, counts each arena's allocations in `Allocator::allocation_counts`, and marks the global allocator's calls for an arena's chunks, which `take_pending_chunk_operation` reports, so a global allocator that counts calls can leave them out.

```rust
use luck_allocator::{Allocator, Box, List};

let mut allocator = Allocator::new();
let boxed = Box::new_in(5, &allocator);
let list = List::from_array_in([1, 2, 3], &allocator);
assert_eq!((*boxed, list.len()), (5, 3));
allocator.reset();
```
