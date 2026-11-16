# luck_benchmark

Internal benchmark harness: one criterion bench binary per pipeline stage, plus corpus management and committed size tracking. Unpublished (`publish = false`, version `0.0.0`).

## Overview

Each stage's benchmark times only that stage, over pinned inputs, so a number moves only when its own stage changes. The bundler benchmark is the one deliberate exception and stays end to end. Run everything locally with `cargo bench -p luck_benchmark`, or one stage with `--bench <stage>`:

```sh
cargo bench -p luck_benchmark --bench parser
```

CI runs the same binaries under [CodSpeed](https://codspeed.io) with the `codspeed` feature, one job per bench binary, in simulation mode on a pinned GitHub-hosted runner. Simulation runs each benchmark under Valgrind, measures one run after the warm-up runs, and estimates its cycles from the instructions it executes plus simulated cache misses, so wall-clock noise doesn't move the numbers. Time spent in system calls isn't counted, so a benchmark that spends much of its time in them, such as on thread handoff, reports less than it costs, and CodSpeed warns about it.

## Benchmarks

There are 50: each stage over the seven inputs below, and the bundler over one module tree.

| ID | Times | Untimed setup |
|----|-------|---------------|
| `lexer[*]` | `luck_lexer::lex` and dropping its result | corpus loading |
| `parser[*]` | `luck_parser::parse` on a `&str`, including the copy into `ParseResult.source` and dropping the result | corpus loading |
| `semantic[*]` | `luck_semantic::analyze_with_environment` with the input's own stdlib environment | parse |
| `linter[*]` | `luck_linter::lint_analyzed` with every category at warn, on a one-thread rayon pool that is the bench thread | parse, semantic analysis, config |
| `codegen[*]` | `luck_codegen::compact_with_capacity` | parse |
| `formatter[*]` | `luck_formatter::format_parsed`: comment placement, require sorting, and printing | parse |
| `minifier[*]` | `luck_minifier::minify_block` on a clone of the tree: the passes, each round's `compact`, and dropping the consumed tree | parse, clone, dropping the output |
| `bundler[gen_modules]` | `luck_bundler::bundle` from disk | none |

The linter's pool keeps its large-file paths, `rayon::join` and `par_chunks`, running inline as one chunk, so the work doesn't wait on other threads or depend on the runner's core count.

| Input | Target | Contents |
|-------|--------|----------|
| `gen_full_lua54.lua` | Lua 5.4 | 40 full-grammar `luck_testgen` programs: every version-gated construct in a realistic statement mix |
| `gen_full_luau.luau` | Luau | The same for Luau |
| `idiomatic.lua` | Lua 5.4 | The `tests/fixtures/idiomatic` files, a small hand-written sample |
| `infinite_yield.luau` | Luau, Roblox | A 13k-line, comment- and string-heavy Roblox admin script |
| `obfuscated_vm.luau` | Luau, Roblox | 2.2 MB of VM-obfuscated Luau on a single line, the adversarial case |
| `roact` | Luau, Roblox | Roact v1.4.4, ~80 files per iteration |
| `penlight` | Lua 5.1 | Penlight 1.15.0, ~40 files per iteration |

`gen_modules` is a 40-module diamond DAG of generated Lua 5.4, because real projects don't bundle cleanly: Penlight requires stdlib modules and builds require strings dynamically, and Roact uses Roblox instance paths.

### Deliberate couplings

- `parser[*]` includes lexing, because the parser drives the lexer.
- `minifier[*]` includes codegen, because each round compacts the tree to tell whether it changed.
- `bundler[gen_modules]` includes everything `bundle` calls: resolution and its disk probes, reading and parsing every module, graph construction, and emit. Like oxc's pipeline benchmark, it's the one end-to-end number.
- `luck build`'s `bundle_minified` has no benchmark of its own. It builds the graph `bundle` does and minifies with `minify_block`, so `bundler` and `minifier` time its parts.

### IDs are a contract

A CodSpeed ID is `crates/luck_benchmark/benches/<file>.rs::<criterion_group>::<function>::<group>[<parameter>]`, such as `crates/luck_benchmark/benches/parser.rs::parser::bench_parser::parser[roact]`. Renaming any part of it, the bench file, the `criterion_group!`, the bench function, the `benchmark_group`, or the `BenchmarkId` parameter, starts a new history and orphans the old one. Internals are free to change under a stable ID.

### Changing what a benchmark measures

A PR that changes what a benchmark times, or the inputs it reads, touches only `luck_benchmark` and the API that change needs, such as `format_parsed` for the formatter. Its CodSpeed steps are then the measurement change alone, and get acknowledged once. A step on a PR that also changes a stage can't be told apart from a regression.

## Corpus

`corpus.rs` pins every input to a commit SHA. Each is fetched once into the gitignored `corpus/` cache (`corpus_cache.rs`) and never generated at run time, so a generator, fixture, or upstream change can't move a number under an unchanged ID.

- **`esoware/luck-bench-corpus`** holds the five single files and the bundler's `gen_bundle/` tree, fetched as one tarball. The generated files there are `luck_testgen` output frozen at one commit, so they don't track the grammar. Refreshing or adding one is a push there and a new `CORPUS_SHA`, which changes what the benchmarks measure.
- **Roact and Penlight** are fetched as tarballs of their upstream repositories.

`test_files()`, `test_projects()`, and `bundle_project_root()` are the entry points the benches and the `metrics` test consume. Each input carries its `LuaTarget`, and `TestFile::parse` and `TestProject::parse_files` parse it outside the measured section, failing on any parse error.

## Size tracking

`tests/it/metrics.rs` maintains `minsize.snap` (modeled on oxc's `tasks/minsize`): exact minified and gzipped byte counts for every corpus input, committed and checked in CI so any size regression or win surfaces as a diff. The check and the regenerator are both `#[ignore]`d because they fetch and minify the whole corpus, and the benchmark workflow runs the check. Regenerate after a minifier or corpus change:

```sh
cargo test -p luck_benchmark --test it regenerate_minsize -- --ignored
```

## Allocator

Benches install `NeverGrowInPlaceAllocator`, a thin wrapper over [`MiMalloc`] (the CLI's production allocator) that leaves out `realloc`. Whether a native `realloc` grows in place or moves depends on the allocator's internal state, which adds a lot of variance between runs. The default `realloc` always allocates, copies, and frees, a consistent worst case that keeps results stable. Benches import criterion through `luck_benchmark`, which links the allocator into every bench binary.

Allocation stays inside the measurement, because cutting allocations is most of luck's performance work. The benchmark workflow sets `MIMALLOC_PURGE_DELAY=-1`, because mimalloc otherwise returns freed memory to the OS on a 10 ms wall-clock timer, which under simulation fires at a different point in every run.

[`MiMalloc`]: https://docs.rs/mimalloc-safe
