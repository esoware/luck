---
name: add-lint-rule
description: Adds a new lint rule to luck_linter end-to-end, covering the rule file, category choice, bus vs whole-tree shape, registration, auto-fix policy, and required tests. Use when asked to add a lint, warn when X, flag Y, detect a pattern, or for any edit under crates/luck_linter/src/rules/.
---

# Add a lint rule

Rules are stateless `Rule` impls emitting `LintDiagnostic`s, and everything they see arrives through `LintContext`. Before writing anything, read one existing rule of similar shape from `crates/luck_linter/src/rules/` and mirror it. The code is the source of truth, not this file.

## 1. Pick a category

`Category` lives in `luck_core` (re-exported as `luck_linter::Category`): `Correctness` (the only default-on category, which requires zero false positives on valid, idiomatic Lua), `Suspicious` (off, with intentional false positives allowed), `Style`, `Performance`. The rule goes in `Suspicious` unless you can prove zero false positives, including on the module pattern (`local t = {} function t:m() end return t`), branch initialization (`local x if c then x = 1 end`), and closures.

## 2. Pick the shape

Either way the `Rule` impl holds only the rule's identity: `name`, `category`, `default_severity`, and `description`.

- **Node-local (preferred).** The rule fires by matching a single statement or expression with no traversal state. Implement `NodeRule`: its hooks and `node_types()`. Omitting a type the hooks match would silently disable the rule for it, though the debug-build dual-dispatch verifier catches the mismatch in any test run. Never recurse inside hooks: the bus walks, and hooks fire once per node. A hook sees only its own node and what hangs below it, such as a function or loop body to run `cfg.rs` analysis on.
- **Whole-tree.** The rule needs traversal state (scope stacks, statement-sequence windows). Implement `WholeRule::check` with an internal `Visitor`, never hand-rolled recursion, which misses nested blocks.

Cross-cutting rules:

- Name/scope questions resolve through `context.semantic`'s scope tree, never by slicing identifier text out of source (breaks on shadowed names).
- Config-driven behavior reads `context.config` at check time. Rules never carry constructor state.
- Build diagnostics with `LintDiagnostic::new(self.name(), "message", span)` plus optional `.with_help(..)`. Rules never set category or severity per diagnostic. The driver stamps them.

## 3. Auto-fix (optional)

Attach a `Fix` only when the rewrite is **always** safe. There is no unsafe-fix tier. If it is sometimes wrong, don't ship it. Edit spans cover exactly the replaced tokens, the edited output must re-parse, and a symbol rename must edit every reference, not just the declaration.

## 4. Register

In `crates/luck_linter/src/rules.rs`: add the `mod` and a `RULES` entry, `RuleEntry::Node` for a node rule and `RuleEntry::Whole` for a whole-tree one, keeping category grouping and mirroring neighboring entries. The `rule_count_locked` test hard-codes the rule count. Bump it and update the files its message names.

## 5. Tests

Inline `#[cfg(test)]` module in the rule's own file using the shared `run_rule` helper: positive cases named `flags_*`, negatives `ignores_*`, count assertions carrying a `"{diags:?}"` message. If a fix ships, add a fix test that applies the edit with `fixtures::fix_and_reparse`, which asserts the result re-parses.

Do NOT write a per-rule suppression test. The `lint()` driver applies suppression, not the rule, and central tests already cover it. Add a driver-level test in `tests/it/directives.rs` only if the rule emits diagnostics whose spans don't sit on the offending statement.

## 6. Gate and bump

`just lint && just test -p luck_linter`. New rule = new feature = minor bump: follow `.agents/skills/bump-versions/SKILL.md`.
