# luck_minifier

AST-level minification for Lua and Luau.

## Overview

`luck_minifier` runs a sequence of AST transforms on a parsed program and emits the result through `luck_codegen` in compact mode. Each pass takes and returns a `Block` and shares no state with the others, so passes compose and can be tested on their own.

## Key features

- **Twelve transform passes.** The classic minifier optimizations plus a few specific to Lua's semantics. Eleven toggle individually through `TransformConfig` (`remove_dead_code`, `simplify_statements`, `fold_constants`, `inline_locals`, `merge_locals`, `simplify_indexes`, `shorten_strings`, `shorten_numbers`, `simplify_parens`, `rename_locals`, `lift_locals`), and `explicit_self` runs whenever `rename_locals` does. The opt-in, off-by-default `rename_globals` flag extends renaming to globals the file defines.
- **Fixpoint pipeline.** `fold_constants` and `remove_dead_code` each run twice within a round, since inlining exposes new folds and folding exposes new dead branches. The whole chain then repeats until the emitted output stops changing.
- **Scope-aware renaming.** `rename_locals` resolves every reference to its binding and lets bindings whose live scopes don't overlap share the same short name.
- **Metamethod-safe.** Purity checks treat a variable read, and arithmetic, comparison, concatenation, or indexing involving one, as impure, because any of them can dispatch through a metamethod with side effects. A literal operation that can raise, such as `1 // 0`, counts as impure too.
- **Idempotent output.** `minify(minify(x)) == minify(x)` is a hard invariant, enforced by tests.

## Architecture

### Pipeline

`minify()` runs the chain to a fixpoint: each outer round runs the core passes below in order, then the tail, and emits the result. The next round starts from a reparse of that output, and the loop stops once the emitted text stops changing (bounded by a small round cap for pathological oscillation).

Core passes, one round, in order:

1. `remove_dead_code` drops unused locals, resolves branches with constant conditions, and trims redundant code such as `local x = nil`, a `for` step of `1`, or a trailing bare `return`.
2. `simplify_statements` merges nested `if`s into one `and` condition and turns trailing `if c then return X end` guards into an `and`/`or` return.
3. `fold_constants` evaluates constant expressions at compile time.
4. `inline_locals` replaces a single-use local whose initializer is a literal expression with that expression.
5. `fold_constants` runs a second time, because inlining exposes new folds.
6. `remove_dead_code` runs a second time, because the newly exposed folds, such as `local DEBUG = false` inlined into `if DEBUG then`, expose new dead branches.
7. `merge_locals` combines adjacent single-name `local` declarations, or adjacent assignments to proven locals, into one statement. A merged assignment stores right to left, so a global or field store, which can run `__newindex`, stays on its own.
8. `simplify_indexes` converts `t["key"]` into `t.key` and `{["key"] = v}` into `{key = v}` when the string is a valid identifier on the target, and drops `[1]`, `[2]`, ... keys that run in sequence.
9. `shorten_strings` re-encodes each string literal as a double-quoted string and keeps whichever spelling is shorter.
10. `shorten_numbers` picks the shortest spelling of each number literal that keeps its value and subtype.
11. `simplify_parens` removes parentheses that precedence already makes redundant, and the call parentheses around a lone string or table argument.

Tail (runs only when `rename_locals` or `lift_locals` is enabled): when renaming, `explicit_self` first rewrites `function X:Y(...)` to `function X.Y(self, ...)` in methods that use `self` at least twice, so the renamer can shorten the parameter. Then `rename_locals`, `lift_locals`, and a `merge_locals` that fuses lifted declarations repeat to their own fixpoint before the outer round continues. That loop doesn't shrink the output monotonically, so it keeps the shortest emitted output it has seen.

### Transforms

Every transform implements `Transform` from `luck_ast`, overriding only the hooks it needs and delegating to `self.walk_*` for the default recursion. Matches over AST variants are exhaustive, so a new variant can't silently bypass a pass.

### Scope and naming

The renamer does its own scope analysis in `rename_locals/analyzer.rs`. It resolves each reference to a binding (a reference that resolves to none is a global), records the scopes each binding is live in, including closures that capture it, and lets bindings whose live scopes don't overlap share a name. The most-referenced bindings get the shortest names.

`rename_locals/candidate_pool.rs` maps an index to a candidate identifier via a single `name_for_index` function, shortest names first. The first characters tried spell `luck`/`LUCK` before falling through the rest of the alphabet (`l`, `u`, `c`, `k`, `L`, `U`, `C`, `K`, `a`, `b`, `d`, ...), and multi-character names mix in digits from the second character on. The renamer's `CandidatePool` walks that sequence, skipping keywords, globals read where the binding is live, and names another binding already wears in an overlapping scope.
