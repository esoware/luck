# luck_minifier

AST-level minification for Lua and Luau.

## Overview

`luck_minifier` runs a sequence of AST transforms on a parsed program and emits the result through `luck_codegen` in compact mode. Each transform is a standalone `fn(Block) -> Block` with no shared mutable state between passes, which makes them composable and individually testable.

## Key Features

- **Twelve transform passes** — every classic minifier optimization plus a few unique to Lua's semantics, all individually toggleable through `TransformConfig` (`remove_dead_code`, `simplify_statements`, `fold_constants`, `inline_locals`, `merge_locals`, `simplify_indexes`, `shorten_strings`, `shorten_numbers`, `simplify_parens`, `rename_locals`, `lift_locals`, and the opt-in, off-by-default `rename_globals`).
- **Two-pass constant folding and local merging** — the pipeline runs these twice to catch opportunities exposed by other transforms.
- **Scope-aware renaming** — `rename_locals` reuses short names safely across non-overlapping scopes, using the same scope analyzer that powers the linter.
- **Metamethod-safe** — purity checks reject arithmetic, comparison, and concatenation involving variables, because any of those can dispatch through a metamethod with side effects.
- **Idempotent output** — `minify(minify(x)) == minify(x)` is a hard invariant, enforced by tests.

## Architecture

### Pipeline

Transforms run in this order. Each is individually toggleable through `TransformConfig`:

1. **`remove_dead_code`** — strips unreachable statements after `return` / `break`.
2. **`simplify_statements`** — flattens unnecessary blocks and simplifies trivial control flow.
3. **`fold_constants`** — evaluates constant expressions at compile time.
4. **`inline_locals`** — substitutes single-use local variables with their initializer.
5. **`fold_constants`** — second pass to catch opportunities exposed by inlining.
6. **`merge_locals`** — combines adjacent `local` declarations.
7. **`simplify_indexes`** — converts `t["key"]` into `t.key` when the string is a valid identifier.
8. **`shorten_strings`** — picks the shortest valid representation of each string literal.
9. **`shorten_numbers`** — picks the shortest valid representation of each number literal.
10. **`simplify_parens`** — removes redundant parenthesization that does not affect grammar.
11. **`rename_locals`** — renames locals to the shortest unused identifiers in each scope.
12. **`lift_locals`** — hoists locals to widen merge opportunities, followed by a final `merge_locals` pass that fuses the lifted declarations.

`rename_locals` runs near the end because anything before its tail would see renamed variables and produce nonsense; only the `lift_locals` / final `merge_locals` cleanup follows it.

### Transforms

Every transform implements `AstTransform` from `luck_ast`, overriding `transform_expression` and/or `transform_statement` and delegating to `self.walk_*` for default recursion. Transforms never reach across nodes manually — recursion goes through the trait so new AST variants do not silently bypass the pass.

### Scope and Naming

The renamer's scope analysis lives inside the `rename_locals` transform itself: which locals exist in each scope, which upvalues each function captures, and which names are safe to reuse in non-overlapping scopes. There is no separate `scope.rs`.

`name_gen.rs` maps an index to the shortest candidate identifier (`a`, `b`, …, `z`, `aa`, `ab`, …) via a single `name_for_index` function; the renamer's `CandidatePool` walks that sequence, skipping keywords, in-use names, and names that would collide within a live scope.
