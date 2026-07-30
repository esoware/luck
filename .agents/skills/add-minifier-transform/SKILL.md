---
name: add-minifier-transform
description: Adds a new AST-level optimization pass to luck_minifier with its config flag, pipeline slot, and metamethod-safety tests. Use when asked to add a minifier pass, shrink output, fold X, rewrite an expression to be smaller, or for any edit under crates/luck_minifier/src/transforms/.
---

# Add a minifier transform

A transform is a `Block -> Block` pass implementing `AstTransform`
(`luck_ast/src/transform.rs`), one file per pass under
`crates/luck_minifier/src/transforms/`. Four layers: write the pass, add its
config flag, wire it into the ordered pipeline, prove it preserves meaning.

## 1. Write the pass

Always recurse through `self.walk_*` - hand-rolled recursion misses nested
cases. Exhaustive matches only, no `_ =>` arms. If the pass needs
scope/binding information, do **not** build a flat name-to-value map - that
miscompiles shadowed names; use a scope-aware analysis (see `rename_locals`'s
Analyzer for the reference implementation) or don't write the pass yet.
Purity questions go through `is_pure_expression` in `src/expr.rs`.

## 2. Config flag

Every pass gets a bool on `TransformConfig`
(`luck_core/src/transform_config.rs`), then `just schema` (the drift test
fails if skipped).

## 3. Wire into the pipeline

`pub mod` in `transforms/mod.rs`, slot in `lib.rs::minify()`. **Read the
current pipeline order from `minify()` before choosing a slot - do not trust
any written snapshot of it.** Structural facts that hold: `fold_constants`
runs twice because inlining exposes new folds (if your pass exposes
opportunities for an earlier pass, re-run that pass after yours);
`rename_locals` is not last, so a pass touching user-visible names must run
before it. Pipeline-order interactions are a known bug source - if your pass
reorders or merges declarations, add a test combining it with `lift_locals`
and `merge_locals`.

## 4. Semantics safety (this is where bugs hide)

Metamethods: identifiers, indexing, arithmetic, comparison, concat, and
length can all dispatch through metamethods with side effects. A transform is
only safe on operands that cannot metamethod:

- `is_pure_expression(_, allow_var_reads=true)` rejects variable arithmetic;
  only literal arithmetic is pure.
- `#"str"` must not be folded - escape sequences make raw length unreliable.
- `-a + b != -(a + b)` for variable `a`; sign folding works only on literals.
- `__lt` vs `__le` differ: never invert comparisons, only wrap in `not`.
- `a == nil` != `not a` (falsy is broader); don't fold equality with nil.
- `a .. b` runs `__concat`; never fold across variables.

Other recurring miscompile classes:

- **Multi-return truncation**: a call or `...` in tail position expands;
  parenthesized or moved to non-tail position it truncates to one value.
- **Integer/float subtype (5.3+)**: `1` and `1.0` differ observably
  (`math.type`, `tostring`, `//`); no version-blind numeric rewrites.
- **String escapes**: token payloads are raw source text, not decoded values;
  never compare or concatenate string literals textually.
- **Attributes**: `<close>` locals have scope-exit side effects and `<const>`
  affects validity; never remove, merge, or move an attributed local.

## 5. Tests (all four)

1. Output shorter or equal - never longer - on a representative fixture.
2. Re-parses: `parse(&minified).errors.is_empty()`.
3. Idempotent: `minify(minify(src)) == minify(src)`.
4. Metamethod-safe: a `setmetatable` fixture the transform must leave alone.

## 6. Gate and bump

`just lint && just test -p luck_minifier -p luck_core`. New transform = minor
bump of `luck_minifier` AND `luck_core` (new config field): follow
`.agents/skills/bump-versions/SKILL.md`.
