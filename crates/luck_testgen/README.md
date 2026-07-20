# luck_testgen

Internal test harness: deterministic Lua/Luau program generators and the
property tests that run over them. Unpublished (`publish = false`,
version `0.0.0`).

## Overview

Two generators turn a `(seed, LuaVersion, statement_budget)` triple into a
source string. Both are fully deterministic - same inputs, byte-identical
output, on every platform - which is what lets the generated programs seed
benchmark corpora and fuzz corpora without committing large files. The two
profiles trade off differently:

| Generator | Entry | Guarantee | Purpose |
|-----------|-------|-----------|---------|
| Runtime-safe | `generate` | Parses **and executes** without error | Differential testing: run original vs. transformed program, compare stdout |
| Full-grammar | `generate_full` | Parses with zero errors (may not execute) | Pipeline stress: reach every version-gated construct |

Determinism is the load-bearing property. A single deterministic xorshift64\*
`Rng` drives every choice; there is no external RNG dependency and no
wall-clock or hashmap-order input.

## Generators

### Runtime-safe (`generate`)

`Generator` tracks a scope stack of typed bindings (`Num`/`Str`/`Bool`/
`Table`) so every emitted operation is type-safe at runtime: no `"a" + 1`,
no calling a number, no division by a literal zero, no reads of an
uninitialized variable. Loops are counter-bounded and library calls avoid
observable nondeterminism (`pairs` order, time, GC). Every top-level
binding is `print`ed at the end, so a transform that wrongly eliminates or
reorders an assignment changes stdout - the signal the differential
harness keys on. A small name pool with a deliberate 30% bare-stem
shadowing rate exercises the shadowing bug class that flat analyses miss.

### Full-grammar (`generate_full`)

`FullGenerator` emits module-shaped code (strict-mode prelude, an OOP class
table with a constructor and methods, then a call- and table-heavy body,
then a module return) rather than statement soup. Output is **not**
runtime-safe: it may index nil, divide strings, or call numbers. In
exchange it reaches constructs the runtime profile cannot - `goto`/labels,
Luau type annotations and casts, named varargs, string interpolation,
compound assignment, attributes, hex/binary literals - each gated behind
the matching `LuaVersion::has_*` predicate so a program never contains a
construct its version rejects.

## Property tests

`tests/roundtrip.rs` runs the workspace's hard invariants over both
generators across every version and 60 seeds:

- **Parse cleanliness** - every generated program parses with zero errors.
- **Compact round-trip** - `compact` output re-parses cleanly.
- **Format idempotency + structure** - `format(format(x)) == format(x)`,
  the output re-parses, and the AST structure is preserved
  (`blocks_equiv`), on both the source-in and AST-in (`format_block`)
  paths.
- **Minify idempotency** - minified output re-parses, size is stable
  across passes, and a byte-exact fixpoint is reached from the second pass
  onward.

These sweeps are the enforcement mechanism behind the formatter and
minifier guarantees; strengthen them rather than bypass them.

## Usage

```rust
use luck_testgen::{generate, generate_full};
use luck_token::LuaVersion;

let runtime = generate(42, LuaVersion::Lua54, 30);
let full = generate_full(42, LuaVersion::Luau, 30);
assert_eq!(runtime, generate(42, LuaVersion::Lua54, 30)); // deterministic
```
