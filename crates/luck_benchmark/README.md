# luck_benchmark

Criterion benchmarks for each stage of the luck pipeline, one binary per stage.

```sh
just bench                 # every stage
just bench --bench parser  # one stage
just minsize               # rewrite the minified-size snapshot
```

CI runs the same binaries on CodSpeed with the `codspeed` feature.
