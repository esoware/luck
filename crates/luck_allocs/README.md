# luck_allocs

Tracks how many heap and arena allocations each stage of the luck pipeline makes.

```sh
just allocs        # rewrite the committed allocs_<stage>.yaml snapshots
just allocs rules  # measure each lint rule alone, into a gitignored report
```

The snapshots are committed, so a change that adds or removes allocations shows up in its diff. Ported from oxc's `tasks/track_memory_allocations`.
