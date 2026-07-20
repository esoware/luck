# luck_linter

Rule-based linter for Lua and Luau with scope-aware analysis.

## Overview

`luck_linter` runs a configurable set of rules against a parsed AST, using semantic analysis from `luck_semantic` for scope-aware diagnostics. Rules can attach safe auto-fixes that the CLI applies under `--fix`. Suppression comments scope to the next statement.

## Key Features

- **64 rules across 4 categories** — Correctness, Suspicious, Style, and Performance. Correctness rules are on by default; the rest are opt-in.
- **Auto-fix** — rules with a safe fix attach `TextEdit`s to their diagnostic, applied in a single pass with overlap detection.
- **Suppression comments** — `-- luck: allow(rule_name)` on the preceding line suppresses the rule for the entire span of the following statement.
- **Semantic-aware** — unused/undefined variable, incorrect stdlib usage, and similar rules query the `ScopeTree` rather than re-walking the AST.

## Rules

### Correctness (on by default)

| Rule | Description |
|------|-------------|
| `unused_variable` | Variables declared but never read |
| `undefined_variable` | References to variables not in scope |
| `unreachable_code` | Statements after an unconditional return or break |
| `unbalanced_assignment` | Assignments where LHS and RHS counts differ |
| `duplicate_keys` | Repeated keys in table constructors |
| `incorrect_stdlib_use` | Wrong argument count or misuse of standard library functions |
| `compare_nan` | Comparisons with NaN (always false) |
| `type_check_inside_call` | `type()` used inside a function call instead of being compared |

### Suspicious (off by default)

| Rule | Description |
|------|-------------|
| `almost_swapped` | Variable swap attempts missing the temporary |
| `constant_table_comparison` | Comparing a fresh table literal with `==` / `~=` |
| `deprecated` | Use of deprecated standard library functions |
| `duplicate_conditions` | Repeated conditions in `if` / `elseif` chains |
| `empty_block` | Empty `if`, `else`, or loop bodies |
| `if_same_then_else` | Identical `then` and `else` branches |
| `setting_global` | Assignments to the global scope |
| `reversed_for_loop` | Numeric `for` loops where the step goes the wrong direction |
| `must_use` | Discarding return values from pure functions |

### Style (off by default)

| Rule | Description |
|------|-------------|
| `parenthesized_conditions` | Unnecessary parentheses around `if` and `while` conditions |
| `shadowing` | Variable declarations that shadow an outer variable |

## Architecture

### Pipeline

`lint()` parses the source, runs `luck_semantic::analyze()` to build the scope tree and resolve stdlib symbols, runs every enabled rule against the AST and analysis, applies suppression comments at the statement-span level, and sorts the resulting diagnostics by position.

### Rules

Every rule implements the `Rule` trait: `name`, `category`, `default_severity`, `description`, and `check`. `check` receives a single `LintContext` — `fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic>` — which bundles the block, the semantic analysis, the source text, the comment array, and the resolved config. Rules that need traversal state (scope stacks, statement sequences, control flow) walk the AST through `Visitor`, never hand-rolled recursion.

Rules whose logic is node-local additionally implement the `NodeRule` trait, which exposes per-node hooks (`on_statement`, `on_expression`) instead of a full walk. The `bus` runs one shared pre-order pass over the AST and fans each node out to every subscribed `NodeRule`, replacing N per-rule traversals with a single one; each node rule's `Rule::check` just delegates to `bus::run_single`.

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
