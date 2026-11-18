# luck_allocs

Internal and unpublished: counts the heap and arena allocations each pipeline stage, the lexer, parser, semantic analysis, linter, codegen, formatter, and minifier, makes on every file in `luck_benchmark_files`, and commits them as one `allocs_<stage>.yaml` per stage. A change that allocates more or less shows up in its own diff, and CI fails when the snapshots are stale. Ported from oxc's `tasks/track_memory_allocations`.

```sh
just allocs        # rewrite the snapshots
just allocs rules  # write the gitignored allocs_linter_rules.yaml instead
```

To cut a stage's allocations, change it, run `just allocs`, and read `git diff crates/luck_allocs`: every count that moved is one line. `just allocs rules` measures each lint rule alone on each file, heaviest first, which shows where the linter's allocations come from. Commit the snapshots with the change that moved them.

## Metrics

For each file and stage:

- `file size`: the source's size
- `sys allocs`, `sys reallocs`, and `sys deallocs`: the system allocator's calls during the stage
- `sys alloc bytes`: the bytes the stage asked the system allocator for, allocation sizes plus realloc growth
- `sys peak growth`: how far live heap memory rose above where it started during the stage, which catches memory a stage frees again before it ends
- `arena allocs`: the stage's arena allocations
- `arena reallocs`: the stage's `List` growths, each of which moves a list to a bigger arena allocation
- `arena size`: the bytes in use in the arena when the stage ends. Every stage shares one arena per file, so this includes what the stages before it left, such as the parser's tree.

Each stage's output is freed inside its own measurement, except the parse, which the stages after it read. The minifier rewrites the tree, so it runs last.

## Across platforms

Arena chunks are left out of every `sys` metric, because whether an arena needs one more chunk depends on byte totals that vary between architectures, and a reused arena's chunks reflect its history rather than the stage. The arena marks each chunk operation for the global allocator to skip, behind `luck_allocator`'s `track_allocations` feature, which no other crate turns on.

Counts are the same everywhere, because the linter's rayon pool is only the measuring thread, and both reports start with a warm-up pass over every file that grows the arena and initializes anything lazy. Byte totals can still move a few bytes between architectures, because some type layouts differ, so `just allocs` keeps a byte value's committed number while the measured one stays within tolerance of it: 1 KiB for `arena size`, and 1% with a 4 KiB floor for `sys alloc bytes` and `sys peak growth`.
