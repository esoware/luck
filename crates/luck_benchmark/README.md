# luck_benchmark

Internal and unpublished: a criterion bench for each pipeline stage, the lexer, parser, semantic analysis, linter, codegen, formatter, minifier, and bundler, over a pinned corpus of real files and projects. The first run downloads the corpus into the gitignored `corpus/` directory. CI runs the benches on CodSpeed with the `codspeed` feature.

```sh
just bench                 # every stage
just bench --bench parser  # one stage
```

`minsize.snap` records the minified and gzipped size of every corpus input, and CI checks it. After a change to the minifier or the corpus, `just minsize` regenerates it.
