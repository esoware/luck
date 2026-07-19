---
name: add-lint-rule
description: Adds a new lint rule to luck_linter end-to-end - rule file, category choice, bus vs whole-tree shape, registration, auto-fix policy, and required tests. Use when asked to add a lint, warn when X, flag Y, lint for Z, detect unused/undefined/duplicate/deprecated patterns, or for any edit under crates/luck_linter/src/rules/.
argument-hint: <rule-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Add a lint rule

A lint rule implements the `Rule` trait and emits `LintDiagnostic`s.
Rules are stateless units: everything they see arrives through
`LintContext` (block, semantic, source, comments, config). Node-local
rules additionally implement `NodeRule` and ride the shared single-pass
bus instead of walking the AST themselves. Auto-fix is optional but must
be 100 % safe when present.

Before writing anything, read one existing rule of similar shape from
`crates/luck_linter/src/rules/` and mirror it - the code is the source of
truth, not this file.

## Layout

| Where | What |
|---|---|
| `crates/luck_linter/src/rule.rs` | `Rule` + `NodeRule` traits, `LintContext` |
| `crates/luck_linter/src/bus.rs` | single-pass dispatch (`run`, `run_single`) for `NodeRule`s |
| `crates/luck_linter/src/diagnostic.rs` | `LintDiagnostic`, `Severity`, `Category`, `Fix`, `TextEdit` |
| `crates/luck_linter/src/rules/` | one file per rule (tests inline in the same file) |
| `crates/luck_linter/src/rules/mod.rs` | static `RULES: &[RuleEntry]` registry |
| `crates/luck_linter/src/fix.rs` | `--fix` apply pass with overlap detection |
| `crates/luck_linter/src/suppression.rs` | `-- luck: allow(...)` directive parser |
| `crates/luck_semantic/src/lib.rs` | `analyze()` - scope tree + stdlib resolution |

## Steps

### 1. Pick a category

`Category` lives in `luck_core` and is re-exported from
`luck_linter::diagnostic`.

| Category | Default | Required guarantee |
|---|---|---|
| `Category::Correctness` | **on** | Zero false positives on valid, idiomatic Lua. |
| `Category::Suspicious` | off | May have intentional false positives. |
| `Category::Style` | off | Subjective. |
| `Category::Performance` | off | Avoidable runtime cost. |

If you can't prove zero false positives - including on the module pattern
(`local t = {} function t:m() end return t`), branch-initialization
(`local x if c then x = 1 end`), and closures - put it in `Suspicious`.
The default-on category has the highest bar.

### 2. Pick the rule's shape: node-local (bus) or whole-tree

**Node-local (preferred)** - the rule fires by pattern-matching a single
statement or expression, with no traversal state (no scope stacks, no
statement-sequence windows, no CFG). Implement `NodeRule` hooks; the
shared bus runs ONE pass over the flat node table (built by
`luck_semantic::nodes`) for all such rules, bucketed by node type, and
`Rule::check` delegates to `bus::run_single` so per-rule tests run
unchanged:

```rust
use luck_ast::expr::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

pub struct $ArgumentsRule;

impl Rule for $ArgumentsRule {
    fn name(&self) -> &'static str { "$ARGUMENTS" }
    fn category(&self) -> Category { Category::Suspicious }
    fn default_severity(&self) -> Severity { Severity::Warning }
    fn description(&self) -> &'static str { "Short user-facing description." }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

impl NodeRule for $ArgumentsRule {
    // REQUIRED in practice: the exact node types the hooks match on.
    // The bus only offers those types to this rule, and files containing
    // none of them skip the rule entirely. Omitting this (the default
    // None) runs the rule against every node - correct but slow.
    // Over-including a type is safe; omitting one the hook matches would
    // silently disable the rule for it - the debug-build dual-dispatch
    // verifier in lint_parsed catches that mismatch in any test run.
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::BinaryOp]);
        Some(&TYPES)
    }

    // Also available: on_statement(&self, stmt, ctx, out).
    fn on_expression(
        &self,
        expr: &Expression,
        ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        // match on `expr`; out.push(LintDiagnostic::new(...)) on hits.
        // Do NOT recurse - the bus walks; hooks fire once per node.
    }
}
```

`NodeType` has one variant per `Statement`/`Expression` variant
(statement/expression calls are `FunctionCallStmt` vs `FunctionCallExpr`).
If the hook needs the enclosing node, use `ctx.nodes` (parent links and
per-node `ScopeId`) instead of promoting the rule to whole-tree.

**Whole-tree** - the rule needs traversal state (scope tracking,
adjacent-statement patterns, control flow). Implement `check` directly
with an internal `Visitor`:

```rust
use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

impl Rule for $ArgumentsRule {
    // ...metadata as above...

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        // ctx.block, ctx.semantic, ctx.source, ctx.comments, ctx.config
        let mut diagnostics = Vec::new();
        // walk ctx.block with Visitor; push LintDiagnostic::new(...) on hits
        diagnostics
    }
}
```

Config-driven behavior (thresholds, allow-lists) reads `ctx.config` at
check time - rules never carry constructor state.

Diagnostics are built with the builder API - `LintDiagnostic` has **no
`Default` impl** and rules never set category/severity per diagnostic
(the driver stamps them):

```rust
LintDiagnostic::new(self.name(), "message text", span)
    .with_help("optional help text")
```

Whole-tree rules walk through `Visitor` - don't hand-roll recursion,
you'll miss nested blocks. Bus rules never recurse at all. For any
name/scope question, resolve through `ctx.semantic`'s scope tree -
**never** by slicing identifier text out of `ctx.source` (that breaks
on shadowed names like `local table = {}`).

### 3. Auto-fix (optional)

Attach a `Fix` only when the rewrite is **always** safe:

```rust
use crate::diagnostic::{Fix, TextEdit};

LintDiagnostic::new(self.name(), "message", span).with_fix(Fix {
    description: "Replace with X".into(),
    edits: vec![TextEdit { span: edit_span, replacement: "X".into() }],
})
```

There is no "unsafe-fix" tier. If the rewrite is sometimes wrong, don't
ship the fix. Fix spans must cover exactly the tokens being replaced -
never a whole statement/body span for an identifier rename - and the
edited output must re-parse (hard invariant 8). If the fix renames a
symbol, it must edit **every** reference, not just the declaration.

### 4. Register

In `crates/luck_linter/src/rules/mod.rs`: add `pub mod $ARGUMENTS;` and
an entry in the static `RULES: &[RuleEntry]` array, keeping category
grouping:

- whole-tree: `RuleEntry::Whole(&$ARGUMENTS::$ArgumentsRule),`
- bus: `RuleEntry::Node(&$ARGUMENTS::$ArgumentsRule, &$ARGUMENTS::$ArgumentsRule),`
  (the same value twice - two vtables, because dyn-upcasting needs
  Rust 1.86 and MSRV is 1.85)

The `rule_count_locked` test in `mod.rs` hard-codes the rule count -
bump it (and the counts it says to update in the READMEs).

### 5. Tests (both required; three if you ship a fix)

Inline `#[cfg(test)]` module **in the rule's own file**, using the
shared `run_rule` helper (mirror any neighboring rule's test module).
Name positive cases `flags_*` and negative cases `ignores_*`, and give
count assertions a `"{diags:?}"` failure message:

```rust
#[test]
fn flags_the_bad_case() { /* expect 1 diagnostic */ }

#[test]
fn ignores_the_good_case() { /* expect 0 diagnostics */ }

// Only if `Fix` was attached:
#[test]
fn fix_produces_expected_output() { /* apply, compare, and re-parse */ }
```

Do NOT write a per-rule suppression test. Suppression (`-- luck:
allow(...)`) is applied by the `lint()` driver, not by `Rule::check`,
so rule-local tests cannot observe it; the machinery is rule-name-
agnostic and covered centrally in `src/suppression.rs` and the
`src/lib.rs` suppression tests. Add a driver-level test in `src/lib.rs`
only if your rule interacts with suppression unusually (e.g. it emits
diagnostics whose spans don't sit on the offending statement).

### 6. Gate

```sh
cargo clippy -p luck_linter --all-targets -- -D warnings
cargo test -p luck_linter
```

### 7. Version bump

Minor bump `luck_linter` (new rule = new feature). Use `/bump-versions`.
