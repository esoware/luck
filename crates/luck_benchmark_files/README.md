# luck_benchmark_files

The pinned Luau files and projects that luck's benchmarks and snapshots
measure.

They download once into the gitignored `cache/` directory next to this crate.
`luck_conformance` fetches its test suite with the same `fetch_bytes` and
`extract_once`.
