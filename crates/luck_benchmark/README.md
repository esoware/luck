# luck_benchmark

Internal benchmark harness: one criterion bench binary per pipeline stage, plus corpus management and committed size tracking. Unpublished (`publish = false`, version `0.0.0`).

## Overview

Each pipeline stage gets its own bench binary so its numbers move independently. Run everything locally with `cargo bench -p luck_benchmark`, or one stage with `--bench <stage>`:

```sh
cargo bench -p luck_benchmark --bench parser
```

CI runs the same binaries under [CodSpeed](https://codspeed.io) with the `codspeed` feature. CodSpeed's simulation mode counts instructions, so wall-clock noise doesn't move the numbers.

| Bench | Measures |
|-------|----------|
| `lexer` | `luck_lexer::lex` |
| `parser` | `luck_parser::parse` with borrowed and recycled owned source |
| `semantic` | `luck_semantic::analyze` (parse excluded) |
| `linter` | `luck_linter::lint_parsed` (parse excluded) |
| `codegen` | `luck_codegen::compact_with_capacity` (parse excluded) |
| `formatter` | `luck_formatter::format_block` (parse and comment build excluded) |
| `minifier` | `luck_minifier::minify` (parse included, fixpoint dominates) |
| `bundler` | `luck_bundler::bundle` over a 40-module diamond DAG |
| `synth` | `luck_ast::Synth` build + source-less emit + format |

### Stable IDs are a contract

Criterion benchmark IDs (`parser/roact`, `minifier/gen_full_lua54.lua`, ...) are the join key for CodSpeed history and saved baselines. **Do not rename or restructure them.** The group name plus the `BenchmarkId` parameter must stay exactly comparable across commits. Internals are free to change under stable IDs.

`parser/<input>` measures the borrowed public entry point, including the allocation and copy into `ParseResult.source`. `parser_owned_source/<input>` moves a `String` into `parse` and recycles the returned source for the next iteration, so no copy lands inside the timed operation. Both include lexing, parsing, and dropping the resulting AST, comments, and diagnostics, so the gap between them is the cost of copying the source.

## Corpus

`corpus.rs` assembles deterministic bench inputs of three kinds:

- **Generated.** Full-grammar `luck_testgen` output at fixed seeds, so the corpus tracks the current grammar, plus the idiomatic fixtures as a small hand-written sample.
- **Real single files.** Mirrored in `esoware/luck-bench-corpus`, fetched by pinned commit SHA (a 13k-line Roblox admin script, and a 2.2 MB single-line obfuscated VM as an adversarial case).
- **Real projects.** Roact (~80 Luau files) and Penlight (~40 Lua 5.1 files), fetched as pinned upstream tarballs.

Everything network-fetched lands in the gitignored `corpus/` cache and is pinned to a SHA, so inputs are immutable run to run. `test_files()`, `test_projects()`, and `bundle_project_root()` are the entry points the benches and `metrics` test consume. Each input carries its `LuaTarget`, and `TestFile::parse` and `TestProject::parse_files` parse it outside the measured section, failing on any parse error.

## Size tracking

`tests/it/metrics.rs` maintains `minsize.snap` (modeled on oxc's `tasks/minsize`): exact minified and gzipped byte counts for every corpus input, committed and checked in CI so any size regression or win surfaces as a diff. The check and the regenerator are both `#[ignore]`d because they fetch and minify the whole corpus, and the benchmark workflow runs the check. Regenerate after a minifier or corpus change:

```sh
cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored
```

## Allocator

Benches install `NeverGrowInPlaceAllocator`, a thin wrapper over [`MiMalloc`] (the CLI's production allocator) that leaves out `realloc`. Whether a native `realloc` grows in place or moves depends on the allocator's internal state, which adds a lot of variance between runs. The default `realloc` always allocates, copies, and frees, a consistent worst case that keeps results stable. Benches import criterion through `luck_benchmark`, which links the allocator into every bench binary, even one like `synth` that needs nothing else from the crate.

[`MiMalloc`]: https://docs.rs/mimalloc-safe
