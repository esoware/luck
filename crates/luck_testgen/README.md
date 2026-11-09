# luck_testgen

Internal test harness: deterministic Lua/Luau program generators, the property tests that run over them, and the walker and dialect rules for the shared fixture tree. Unpublished (`publish = false`, version `0.0.0`).

## Overview

Two generators turn a `(seed, LuaVersion, statement_budget)` triple into a source string. Both are fully deterministic: same inputs, byte-identical output, on every platform. That's what lets the benchmark corpus include generated programs without committing large files. The two profiles trade off differently:

| Generator | Entry | Guarantee | Purpose |
|-----------|-------|-----------|---------|
| Runtime-safe | `generate_runtime_safe` | Parses **and executes** without error | Differential testing: run original vs. transformed program, compare stdout |
| Full-grammar | `generate_full` | Parses with zero errors (may not execute) | Pipeline stress: reach every version-gated construct |

Determinism matters most. A single xorshift64\* `Rng` (`rng.rs`) drives every choice, with no external RNG dependency and no wall-clock or hash-map-order input.

## Generators

### Runtime-safe (`generate_runtime_safe`)

`RuntimeGenerator` (`runtime.rs`) tracks a scope stack of typed bindings (`Number`/`String`/`Boolean`/`Table`) so every emitted operation is type-safe at runtime: no `"a" + 1`, no calling a number, no division by a literal zero, no reads of an uninitialized variable. Loops are counter-bounded and library calls avoid observable nondeterminism (`pairs` order, time, GC). Every top-level binding still visible at the end is `print`ed, so a transform that wrongly eliminates or reorders an assignment changes stdout, which is what a differential run compares. A small name pool with a deliberate 30% bare-stem shadowing rate exercises the shadowing bug class that flat analyses miss.

### Full-grammar (`generate_full`)

`FullGenerator` (`full.rs`, with its statements, expressions, and types under `full/`) emits module-shaped code (a prelude, an OOP class table with a constructor and a method, then a call- and table-heavy body, then a module return) rather than statement soup. Output is **not** runtime-safe: it may index nil, divide strings, or call numbers. In exchange it reaches constructs the runtime profile can't: `goto`/labels, Luau type annotations and casts, named varargs, string interpolation, every compound assignment operator, attributes, and hex-float and binary literals. Each is gated behind the matching `LuaVersion` predicate, so a program never contains a construct its version rejects.

`generate_full_embeddable` never picks the Luau shape that exports values instead of returning a table, since exports are only valid at module scope. Its output can sit inside a `do ... end` block, which is how the bench corpus concatenates programs.

## Property tests

`tests/it/roundtrip.rs` runs the workspace's hard invariants over both generators, across every version and many seeds:

- **Parse cleanliness.** Every generated program parses with zero errors.
- **Compact round-trip.** `compact` output re-parses cleanly.
- **Format idempotency and structure.** `format(format(x)) == format(x)`, the output re-parses, and the AST structure survives (`blocks_equivalent`).
- **AST-in format.** `format_block` on the bare parsed AST, with no source or comments, re-parses and keeps the structure.
- **Minify fixpoint.** Minified output re-parses, size is stable across passes, and a byte-exact fixpoint arrives from the second pass onward.

These sweeps back the formatter and minifier guarantees. Strengthen them instead of working around them.

## Fixtures

`fixtures.rs` holds the one definition of the shared `tests/fixtures/` tree: `collect_fixture_files` walks it, `detect_version` reads a fixture's dialect from its path, and `is_error_fixture` marks the deliberately invalid `errors/` trees. The parser, codegen, formatter, and lexer harnesses all dev-depend on it, so a new dialect directory is one edit.

## Usage

```rust
use luck_testgen::{generate_full, generate_runtime_safe};
use luck_token::LuaVersion;

let runtime = generate_runtime_safe(42, LuaVersion::Lua54, 30);
let full = generate_full(42, LuaVersion::Luau, 30);
assert_eq!(runtime, generate_runtime_safe(42, LuaVersion::Lua54, 30)); // deterministic
```
