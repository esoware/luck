# luck_benchmark

Criterion benchmarks for each stage of luck's pipeline.

Each stage has its own bench binary, so its numbers move independently of the
others. Run them all with `just bench`, or one with `just bench --bench parser`.
`just minsize` rewrites the committed minified-size snapshot.

## Cargo features

* `codspeed` - Runs the benchmarks under CodSpeed, as CI does.
