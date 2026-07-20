# luck_benchmark

Internal benchmark harness: one criterion bench binary per pipeline stage,
plus corpus management and committed size tracking. Unpublished
(`publish = false`, version `0.0.0`).

## Overview

Each pipeline stage gets its own bench binary so its numbers move
independently. Run everything locally with `cargo bench -p luck_benchmark`,
or one stage with `--bench <stage>`:

```sh
cargo bench -p luck_benchmark --bench parser
```

CI runs the same binaries under [CodSpeed](https://codspeed.io) via the
`codspeed` feature, which measures instruction counts and is immune to
wall-clock drift.

| Bench | Measures |
|-------|----------|
| `lexer` | `luck_lexer::lex` |
| `parser` | `luck_parser::parse` |
| `semantic` | `luck_semantic::analyze` (parse excluded) |
| `linter` | `luck_linter::lint_parsed` (parse excluded) |
| `codegen` | `luck_codegen::compact` (parse excluded) |
| `formatter` | `luck_formatter::format_block` (parse and comment build excluded) |
| `minifier` | `luck_minifier::minify` (parse included; fixpoint dominates) |
| `bundler` | `luck_bundler::bundle` over a 40-module diamond DAG |
| `synth` | `luck_ast::synth` build + source-less emit + format |

### Stable IDs are a contract

Criterion benchmark IDs (`parser/roact`, `minifier/gen_full_lua54.lua`, …)
are the join key for CodSpeed history and saved baselines. **Do not rename
or restructure them** - the group name plus the `BenchmarkId` parameter
must stay exactly comparable across commits. Internals are free to change
under stable IDs.

## Corpus

`corpus.rs` assembles deterministic bench inputs of three kinds:

- **Generated** - full-grammar `luck_testgen` output at fixed seeds, so the
  corpus tracks the current grammar, plus the idiomatic fixtures as a small
  hand-written sample.
- **Real single files** - mirrored in `esoware/luck-bench-corpus`, fetched
  by pinned commit SHA (a 13k-line Roblox admin script; a 2.2 MB
  single-line obfuscated VM as an adversarial case).
- **Real projects** - Roact (~80 Luau files) and Penlight (~40 Lua 5.1
  files), fetched as pinned upstream tarballs.

Everything network-fetched lands in the gitignored `corpus/` cache and is
pinned to a SHA, so inputs are immutable run to run. `test_files()`,
`test_projects()`, and `bundle_project_root()` are the entry points the
benches and `metrics` test consume.

## Size tracking

`tests/metrics.rs` maintains `minsize.snap` (modeled on oxc's
`tasks/minsize`): exact minified and gzipped byte counts for every corpus
input, committed and checked in CI so any size regression or win surfaces
as a diff. Both tests are `#[ignore]`d because they fetch and minify the
whole corpus; the benchmark workflow runs the check. Regenerate after a
minifier or corpus change:

```sh
cargo test -p luck_benchmark --test metrics regenerate_minsize -- --ignored
```

## Allocator

Benches install `NeverGrowInPlaceAllocator`, a thin wrapper over
[`MiMalloc`] (the CLI's production allocator) that deliberately omits
`realloc`. A native `realloc` may grow in place or move depending on the
allocator's internal state, which is nondeterministic and adds large
variance; omitting it forces the default never-grow-in-place path - the
consistent worst case, so results are stable.

[`MiMalloc`]: https://docs.rs/mimalloc-safe
