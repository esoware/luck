---
name: add-minifier-transform
description: Adds a new AST-level optimization pass to luck_minifier (constant folding, dead code removal, local merging, identifier shortening, peephole rewrites) with its config flag, pipeline slot, and metamethod-safety tests. Use when asked to add a minifier pass, new optimization, shrink output, fold X, shorten Y, rewrite an expression to be smaller, or for any edit under crates/luck_minifier/src/transforms/.
argument-hint: <transform-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Add a minifier transform

A minifier transform is a `Block -> Block` function implementing
`AstTransform`. The work happens in four layers: write the pass, add its
config flag, wire it into the ordered pipeline, prove it preserves
meaning.

## Layout

| Where | What |
|---|---|
| `crates/luck_minifier/src/lib.rs` | `minify()` - the pipeline orchestrator |
| `crates/luck_minifier/src/transforms/` | one file per pass |
| `crates/luck_minifier/src/transforms/mod.rs` | pass re-exports |
| `crates/luck_minifier/src/expr.rs` | purity / side-effect helpers |
| `crates/luck_minifier/src/name_gen.rs` | mixed-radix short-identifier generator |
| `crates/luck_core/src/transform_config.rs` | `TransformConfig` - one bool flag per pass |
| `crates/luck_minifier/tests/correctness.rs` | re-parse + idempotency harness |
| `crates/luck_ast/src/transform.rs` | `AstTransform` trait + `walk_*` defaults |

## Steps

### 1. Write the pass

Create `crates/luck_minifier/src/transforms/$ARGUMENTS.rs`. Implement
`AstTransform`. Always recurse through `self.walk_*` - hand-rolled
recursion misses nested cases (`if-elseif-else` chains, `repeat-until`
condition scope, function bodies inside table fields). Exhaustive
matches only - no `_ => ...` arms (hard invariant 3).

If the pass needs scope/binding information (which names resolve where,
what a closure captures), do **not** build a flat name->something map -
that whole approach miscompiles shadowed names. Use a scope-aware
analysis (see `rename_locals`'s Analyzer for the reference
implementation) or don't write the pass yet.

### 2. Add the config flag

Every pass is gated by a bool on `TransformConfig` in
`crates/luck_core/src/transform_config.rs`. Add the field (serde +
schemars derive it into the VS Code schema), then regenerate:

```sh
cargo test -p luck_core regenerate_luckrc_schema -- --ignored
```

The schema drift test fails if you skip this.

### 3. Re-export and wire into the pipeline

Add `pub mod $ARGUMENTS;` to `transforms/mod.rs`, then slot the pass into
`lib.rs::minify()`. **Read the current pipeline order from
`lib.rs::minify()` before choosing a slot - do not trust any written
snapshot of the order, including old versions of this file.** Structural
facts that hold:

- `fold_constants` runs twice (before and after `inline_locals`) because
  inlining exposes new folds. If your pass exposes opportunities for an
  earlier pass, re-run that earlier pass after yours.
- `rename_locals` is **not** last - `lift_locals` and a final
  `merge_locals` run after it. A pass that operates on user-visible
  names must run before `rename_locals`; a pass that operates purely on
  structure may run after, but must not assume meaningful names.
- Pipeline-order interactions are a known bug source (lift+merge once
  broke recursive `local function`). If your pass reorders or merges
  declarations, add a test combining it with `lift_locals` and
  `merge_locals`.

### 4. Metamethod safety (this is where bugs hide)

Lua identifiers, indexing, arithmetic, comparison, concat, and length can
all dispatch through metamethods with side effects. The transform is only
safe on operands that **cannot** metamethod.

- Use `is_pure_expression(_, allow_var_reads=true)` to guard. It rejects
  variable arithmetic; only literal arithmetic is pure.
- `#"str"` must **not** be folded - escape sequences make raw length
  unreliable.
- `-a + b != -(a + b)` for variable `a`. Sign folding works only on literals.
- Comparison operators differ by metamethod (`__lt` vs `__le`). Never
  invert; only wrap in `not`.
- `a == nil` != `not a` (falsy is broader). Don't fold equality with nil.
- `a .. b` runs `__concat`; never fold across variables.

Beyond metamethods, the recurring miscompile classes to check your pass
against:

- **Multi-return truncation**: `f()` in last position expands; wrapped in
  parens or moved to non-tail position it truncates to 1 value. Any
  rewrite that moves a call/`...` in or out of tail position changes
  behavior.
- **Integer/float subtype (5.3+)**: `1` and `1.0` differ observably
  (`math.type`, `tostring`, `//`). Don't rewrite numeric literals without
  version awareness.
- **String escapes**: token payloads are raw source text, not decoded
  values. Never compare or concatenate string literals textually.
- **Attributes**: `<close>` locals have scope-exit side effects; `<const>`
  affects validity. Never remove/merge/move a local that has an attribute.

### 5. Tests (all four must pass)

In the pass's own file or `tests/correctness.rs`:

1. **Shorter (or equal - never longer)** on a representative fixture.
2. **Re-parses.** `parse(&minified).errors.is_empty()`.
3. **Idempotent.** `minify(minify(src)) == minify(src)`.
4. **Metamethod-safe.** A fixture using `setmetatable` that your transform
   must leave alone.

If a differential-execution harness exists in the repo (check
`tests/`), add your repro cases to it - executing original vs minified
under a real interpreter is the only test that catches semantic drift.

### 6. Gate

```sh
cargo clippy -p luck_minifier --all-targets -- -D warnings
cargo test -p luck_minifier
cargo test -p luck_core       # schema drift test
```

### 7. Version bump

**Minor** bump `luck_minifier` (new transform = new feature) **and**
minor bump `luck_core` (new `TransformConfig` field = new config
surface). Use `/bump-versions`.
