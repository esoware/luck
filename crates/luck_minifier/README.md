# luck_minifier

AST-level minification for Lua and Luau.

## Overview

`luck_minifier` runs a sequence of AST transforms on a parsed program and emits the result through `luck_codegen` in compact mode. Each transform is a standalone `fn(Block) -> Block` with no shared mutable state between passes, which makes them composable and individually testable.

## Key features

- **Twelve transform passes.** Every classic minifier optimization plus a few unique to Lua's semantics, all individually toggleable through `TransformConfig` (`remove_dead_code`, `simplify_statements`, `fold_constants`, `inline_locals`, `merge_locals`, `simplify_indexes`, `shorten_strings`, `shorten_numbers`, `simplify_parens`, `rename_locals`, `lift_locals`, and the opt-in, off-by-default `rename_globals`).
- **Fixpoint pipeline.** `fold_constants` and `remove_dead_code` each run twice within a round, since inlining exposes new folds and folding exposes new dead branches. The whole chain then iterates until the emitted output stops changing, rather than running one fixed pass.
- **Scope-aware renaming.** `rename_locals` reuses short names safely across non-overlapping scopes, using the same scope analyzer the linter uses.
- **Metamethod-safe.** Purity checks reject arithmetic, comparison, and concatenation involving variables, because any of those can dispatch through a metamethod with side effects.
- **Idempotent output.** `minify(minify(x)) == minify(x)` is a hard invariant, enforced by tests.

## Architecture

### Pipeline

`minify()` runs the chain to a fixpoint, not a single fixed pass: each outer round runs the core passes below in order, then the tail, re-emits, and reparses its own output; the loop stops once the emitted text stops changing (bounded by a small round cap for pathological oscillation). Each pass is individually toggleable through `TransformConfig`.

Core passes, one round, in order:

1. `remove_dead_code` strips unreachable statements after `return` / `break`.
2. `simplify_statements` flattens unnecessary blocks and simplifies trivial control flow.
3. `fold_constants` evaluates constant expressions at compile time.
4. `inline_locals` substitutes single-use local variables with their initializer.
5. `fold_constants` runs a second time; inlining exposes new folds.
6. `remove_dead_code` runs a second time; the newly exposed folds (e.g. `local DEBUG = false` inlined into `if DEBUG then`) expose new dead branches.
7. `merge_locals` combines adjacent `local` declarations.
8. `simplify_indexes` converts `t["key"]` into `t.key` when the string is a valid identifier.
9. `shorten_strings` picks the shortest valid representation of each string literal.
10. `shorten_numbers` picks the shortest valid representation of each number literal.
11. `simplify_parens` removes redundant parenthesization that does not affect grammar.

Tail (runs only when `rename_locals` or `lift_locals` is enabled): `explicit_self` first rewrites `function X:Y(...)` to `function X.Y(self, ...)` so the renamer can shorten the parameter, then `rename_locals`, `lift_locals`, and a fusing `merge_locals` iterate internally to their own fixpoint (tracking the shortest emitted output seen, since the loop does not improve monotonically) before the outer round continues. `rename_locals` runs late in the tail because anything earlier in the chain would see renamed variables and produce nonsense.

### Transforms

Every transform implements `AstTransform` from `luck_ast`, overriding `transform_expression` and/or `transform_statement` and delegating to `self.walk_*` for default recursion. Transforms never reach across nodes manually; recursion goes through the trait so new AST variants do not silently bypass the pass.

### Scope and naming

The renamer's scope analysis lives inside the `rename_locals` transform itself: which locals exist in each scope, which upvalues each function captures, and which names are safe to reuse in non-overlapping scopes. There is no separate `scope.rs`.

`name_gen.rs` maps an index to the shortest candidate identifier via a single `name_for_index` function, shortest names first; the first characters tried spell `luck`/`LUCK` before falling through the rest of the alphabet (`l`, `u`, `c`, `k`, `L`, `U`, `C`, `K`, `a`, `b`, `d`, ...), and multi-character names mix in digits from the second character on. The renamer's `CandidatePool` walks that sequence, skipping keywords, in-use names, and names that would collide within a live scope.
