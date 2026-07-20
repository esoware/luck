# Pre-rewrite baseline

Captured 2026-07-20 on a clean tree at commit 02a7cf4 (main), before the
crate-by-crate rewrite. Compare against this after each crate lands.

## Tests

- `cargo nextest run --workspace`: 2000 passed, 4 skipped, 0 failed (8.1s).
- `cargo test --doc --workspace`: all doctests pass (1-2 per published crate).

## Benchmarks

`cargo bench -p luck_benchmark -- --save-baseline pre-rewrite` - the
criterion baseline named `pre-rewrite` is saved under `target/criterion/`.
To diff after a change: `cargo bench -p luck_benchmark -- --baseline pre-rewrite`.

Mean times at capture:

| Benchmark | gen_full_lua54 | gen_full_luau | idiomatic | infinite_yield | obfuscated_vm | penlight | roact |
|---|---|---|---|---|---|---|---|
| lexer | 1.514 ms | 1.833 ms | 3.53 us | 1.593 ms | 607.31 us | 964.25 us | 622.48 us |
| parser | 3.334 ms | 4.240 ms | 8.74 us | 3.417 ms | 975.33 us | 2.016 ms | 1.346 ms |
| semantic | 1.870 ms | 2.138 ms | 3.62 us | 3.025 ms | 148.52 us | 1.108 ms | 649.61 us |
| linter | 11.149 ms | 15.705 ms | 60.30 us | 21.118 ms | 1.605 ms | 10.520 ms | 5.942 ms |
| codegen | 842.10 us | 1.104 ms | 1.65 us | 805.95 us | 199.89 us | 474.64 us | 314.45 us |
| formatter | 5.702 ms | 7.326 ms | 15.43 us | 6.091 ms | 5.890 ms | 3.914 ms | 2.288 ms |
| minifier | 91.178 ms | 97.873 ms | 163.19 us | 70.035 ms | 45.688 ms | 35.244 ms | 26.316 ms |

| Benchmark | mean |
|---|---|
| bundler/gen_modules | 4.222 ms |
| synth/build | 124.63 us |
| synth/codegen | 84.02 us |
| synth/format | 565.00 us |

## Minified size (committed minsize.snap)

| Original | Minified | Gzip | Ratio | File |
|---|---|---|---|---|
| 384099 | 235639 | 58411 | 61.35% | gen_full_lua54.lua |
| 469923 | 299447 | 74444 | 63.72% | gen_full_luau.luau |
| 2124 | 959 | 477 | 45.15% | idiomatic.lua |
| 482971 | 378978 | 76001 | 78.47% | infinite_yield.luau |
| 2290215 | 2284872 | 1763350 | 99.77% | obfuscated_vm.luau |
| 254127 | 130007 | 40315 | 51.16% | roact (79 files) |
| 424691 | 130745 | 50656 | 30.79% | penlight (39 files) |

## Workspace shape at capture

17 crates, ~73,600 LOC of Rust; largest files: `luck_cli/src/cli.rs`
(2,896), `luck_ast/src/synth.rs` (2,310), `luck_minifier/src/transforms/
rename_locals.rs` (1,649). 4 `#[allow]` attributes workspace-wide; no
`unwrap()` outside test code except 8 in luck_ast, 34 in luck_linter, and
single digits elsewhere.
