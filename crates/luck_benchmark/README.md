# luck_benchmark

Internal and unpublished: a criterion bench for each pipeline stage, the lexer, parser, semantic analysis, linter, codegen, formatter, minifier, and bundler, over `luck_benchmark_files`'s pinned real files and projects. CI runs the benches on CodSpeed with the `codspeed` feature.

```sh
just bench                 # every stage
just bench --bench parser  # one stage
```

`minsize.snap` records the minified and gzipped size of every benchmark file and project, and CI checks it. Each file is minified twice, and a second pass that changes the output fails the check. After a change to the minifier or the benchmark files, `just minsize` regenerates it.
