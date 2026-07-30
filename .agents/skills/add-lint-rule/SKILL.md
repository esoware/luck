---
name: add-lint-rule
description: Adds a new lint rule to luck_linter end-to-end - rule file, category choice, bus vs whole-tree shape, registration, auto-fix policy, and required tests. Use when asked to add a lint, warn when X, flag Y, detect a pattern, or for any edit under crates/luck_linter/src/rules/.
---

# Add a lint rule

Rules are stateless `Rule` impls emitting `LintDiagnostic`s; everything they
see arrives through `LintContext`. Before writing anything, read one existing
rule of similar shape from `crates/luck_linter/src/rules/` and mirror it -
the code is the source of truth, not this file.

## 1. Pick a category

`Category` lives in `luck_core` (re-exported from `luck_linter::diagnostic`):
`Correctness` (the only default-on category; requires zero false positives on
valid, idiomatic Lua), `Suspicious` (off; intentional false positives
allowed), `Style`, `Performance`. If you can't prove zero false positives -
including on the module pattern (`local t = {} function t:m() end return t`),
branch initialization (`local x if c then x = 1 end`), and closures - the
rule goes in `Suspicious`.

## 2. Pick the shape

- **Node-local (preferred)**: the rule fires by matching a single statement
  or expression with no traversal state. Implement the `NodeRule` hooks and
  declare `node_types()` - omitting a type the hooks match would silently
  disable the rule for it; the debug-build dual-dispatch verifier catches the
  mismatch in any test run. `Rule::check` delegates to `bus::run_single`.
  Never recurse inside hooks - the bus walks, hooks fire once per node. If a
  hook needs the enclosing node, use `ctx.nodes` (parent links) instead of
  promoting the rule to whole-tree.
- **Whole-tree**: the rule needs traversal state (scope stacks,
  statement-sequence windows, CFG). Implement `check` directly with an
  internal `Visitor` - never hand-rolled recursion, which misses nested
  blocks.

Cross-cutting rules:

- Name/scope questions resolve through `ctx.semantic`'s scope tree, never by
  slicing identifier text out of source (breaks on shadowed names).
- Config-driven behavior reads `ctx.config` at check time; rules never carry
  constructor state.
- Build diagnostics with `LintDiagnostic::new(self.name(), "message", span)`
  plus optional `.with_help(..)`. Rules never set category or severity per
  diagnostic - the driver stamps them.

## 3. Auto-fix (optional)

Attach a `Fix` only when the rewrite is **always** safe - there is no
unsafe-fix tier; if it is sometimes wrong, don't ship it. Edit spans cover
exactly the replaced tokens, the edited output must re-parse, and a symbol
rename must edit every reference, not just the declaration.

## 4. Register

In `crates/luck_linter/src/rules/mod.rs`: add the `pub mod` and a `RULES`
entry, keeping category grouping and mirroring neighboring entries (node
rules pass the same value twice - two vtables). The `rule_count_locked` test
hard-codes the rule count; bump it and update the files its message names.

## 5. Tests

Inline `#[cfg(test)]` module in the rule's own file using the shared
`run_rule` helper: positive cases named `flags_*`, negatives `ignores_*`,
count assertions carrying a `"{diags:?}"` message, plus - only if a fix
ships - a fix test that applies the edit and asserts the result re-parses.

Do NOT write a per-rule suppression test: suppression is applied by the
`lint()` driver, not `Rule::check`, and is covered centrally. Add a
driver-level test in `src/lib.rs` only if the rule emits diagnostics whose
spans don't sit on the offending statement.

## 6. Gate and bump

`just lint && just test -p luck_linter`. New rule = new feature = minor bump:
follow `.agents/skills/bump-versions/SKILL.md`.
