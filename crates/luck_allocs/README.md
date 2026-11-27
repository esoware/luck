# luck_allocs

Counts the heap and arena allocations each stage of luck's pipeline makes.

`just allocs` runs every stage over the benchmark files and rewrites the
committed `allocs_<stage>.yaml` snapshots, so a change that adds or removes
allocations shows up in its diff. `just allocs rules` measures each lint rule
alone, into a gitignored report. Ported from oxc's
`tasks/track_memory_allocations`.
