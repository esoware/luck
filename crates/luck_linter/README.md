# luck_linter

Rule-based linter for Lua and Luau with scope-aware analysis.

## Overview

`luck_linter` runs a configurable set of rules against a parsed AST, using semantic analysis from `luck_semantic` for scope-aware diagnostics. Rules can attach safe auto-fixes that the CLI applies under `--fix`. Suppression comments scope to the next statement.

## Key Features

- **Rules grouped into 4 categories** — Correctness, Suspicious, Style, and Performance (`luck_core::Category`). Correctness rules run by default; Suspicious, Style, and Performance rules are opt-in, enabled per-rule or by enabling the whole category in `luck.json`.
- **Auto-fix** — rules with a safe fix attach `TextEdit`s to their diagnostic, applied in a single pass with overlap detection.
- **Suppression comments** — `-- luck: allow(rule_name)` on the preceding line suppresses the rule for the entire span of the following statement.
- **Semantic-aware** — unused/undefined variable, incorrect stdlib usage, and similar rules query the `ScopeTree` rather than re-walking the AST.

## Rules

Each rule lives in its own file under `src/rules/`, one module per rule
(`unused_variable.rs`, `deprecated.rs`, ...), and is registered in the
`RULES` array in `src/rules/mod.rs` - the authoritative list of every rule,
its category, and its default severity.

## Architecture

### Pipeline

`lint()` parses the source, runs `luck_semantic::analyze()` to build the scope tree and resolve stdlib symbols, runs every enabled rule against the AST and analysis, applies suppression comments at the statement-span level, and sorts the resulting diagnostics by position.

### Rules

Every rule implements the `Rule` trait: `name`, `category`, `default_severity`, `description`, and `check`. `check` receives a single `LintContext` — `fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic>` — which bundles the block, the semantic analysis, the source text, the comment array, and the resolved config. Rules that need traversal state (scope stacks, statement sequences, control flow) walk the AST through `Visitor`, never hand-rolled recursion.

Rules whose logic is node-local additionally implement the `NodeRule` trait, which exposes per-node hooks (`on_statement`, `on_expression`, `on_last_statement`) instead of a full walk. The `bus` runs one shared pre-order pass over the AST and fans each node out to every subscribed `NodeRule`, replacing N per-rule traversals with a single one; each node rule's `Rule::check` just delegates to `bus::run_single`.

### Auto-Fix

A rule with an always-safe transformation attaches a `Fix` to its diagnostic. `Fix` holds a list of `TextEdit`s with `(Span, String)` pairs. Under `--fix`, the CLI sorts edits, detects overlaps, and applies non-overlapping ones in a single pass. Fixes that are sometimes wrong are not shipped — there is no unsafe-fix tier.

### Module Layout

| File | Purpose |
|------|---------|
| `lib.rs` | `lint()` entry point and orchestration |
| `rule.rs` | `Rule` and `NodeRule` traits, `LintContext` |
| `bus.rs` | Single-pass dispatch that drives every `NodeRule` in one shared walk |
| `cfg.rs` | Control-flow graph over statement slices, used by some rules |
| `diagnostic.rs` | `LintDiagnostic`, `Severity`, `Category`, `Fix` |
| `suppression.rs` | Comment-based suppression logic |
| `fix.rs` | Auto-fix application via `TextEdit` |
| `format_pattern.rs` | Validators for `string.format`/pattern/`string.pack` literals |
| `path.rs` | Dotted `Name(.Name)*` path extraction shared by stdlib-resolving rules |
| `roblox.rs` | Shared helpers for the Roblox `<Global>.new(...)` constructor rules |
| `suggest.rs` | Levenshtein "did you mean" distance shared by the suggestion rules |
| `rules/*.rs` | Individual rule implementations |

### Testing

Each rule file carries its own `#[cfg(test)] mod tests` (positive cases named `flags_*`, negative cases `ignores_*`) built through the shared `test_support::run_rule` helper. `src/lib.rs` has driver-level tests for config resolution, suppression, and auto-fix. `tests/idiomatic_fixtures.rs` asserts the shared `tests/fixtures/idiomatic/` corpus stays lint-clean.
