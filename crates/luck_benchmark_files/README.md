# luck_benchmark_files

Internal and unpublished: the pinned files and projects that `luck_benchmark`'s benches and `minsize.snap`, and `luck_allocs`'s allocation snapshots, measure. Three single files and a 40-module bundle tree come from esoware/luck-benchmark-files, and two real projects, Roact and Penlight, from their upstream tarballs. Each is pinned to a commit SHA, and the first run downloads them into the gitignored `cache/` directory.

`fetch_bytes`, which retries the transient connection resets CI runners see, also downloads `luck_conformance`'s suite.
