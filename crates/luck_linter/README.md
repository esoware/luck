# luck_linter

Rule-based linter for Lua and Luau with scope-aware analysis.

## Overview

`luck_linter` runs a configurable set of rules against a parsed AST, using semantic analysis from `luck_semantic` for scope-aware diagnostics. Rules can attach safe auto-fixes, which `luck lint --fix` and the language server's code actions apply. Comment directives can suppress a rule or change its severity for one statement, a region, or the whole file.

## Key features

- **Four categories.** Correctness, Suspicious, Style, and Performance (`luck_core::Category`). Correctness rules run by default. The other three are opt-in, enabled per rule or by enabling the whole category in `luck.json`.
- **Auto-fix.** Rules with a safe fix attach `TextEdit`s to their diagnostic, and `apply_fixes` applies them in a single pass, skipping overlapping edits.
- **Suppression comments.** `-- luck: allow(rule_name)` suppresses the rule for the statement that follows, or for the statement it trails on the same line. `deny(...)` and `warn(...)` set the severity instead, `allow(...) start` and `allow(...) end` bound a region, `-- #luck: allow(...)` covers the whole file, and `*` matches every rule.
- **Semantic-aware.** Unused/undefined variable, incorrect stdlib usage, and similar rules query `luck_semantic`'s `ScopeTree` and stdlib resolution instead of tracking scopes themselves.

## Rules

Each rule lives in its own file under `src/rules/`, one module per rule (`unused_variable.rs`, `deprecated.rs`, ...), and is registered in the `RULES` array in `src/rules.rs`. Each rule's category and default severity come from its `impl_rule!` invocation, and `luck_linter::rules()` lists every registered rule.

`global_usage` warns about the global environment tables: `_G` everywhere, and `shared` only under the Roblox stdlib, where it's a global rather than an ordinary name. `no_implicit_globals` is the separate, broader policy: it flags every unresolved name, stdlib globals included, except configured `extra_globals`. Both are opt-in Style rules.

Migrating from 0.2.x: `global_usage` named that broader policy through 0.2 and now has the narrow `_G`/`shared` meaning above. A `luck.json` that enables `global_usage` keeps linting, with the narrower rule. Configs that want the old behavior must rename the override to `no_implicit_globals`. The old name is still registered, so `unknown_rule_names` can't flag it for you.

## Architecture

### Pipeline

`lint()` parses the source. If it doesn't parse, each parse error comes back as a `parse_error` diagnostic and no rules run. Otherwise it runs `luck_semantic` analysis to build the scope tree and resolve stdlib symbols, flattens the AST into node and block tables, runs every enabled rule, turns each rule's findings into diagnostics with the rule's name, its category, and its severity from the config override or the rule's default, applies suppression directives, and sorts the diagnostics by position. `lint_target()` takes a `LuaTarget` so Roblox globals resolve, `lint_parsed()` lints a parse the caller already has, and `lint_analyzed()` lints a parse and analysis the caller already has, which the language server uses.

### Rules

Every rule implements the `Rule` trait through `impl_rule!`, which holds its identity: `name`, `category`, `default_severity`, and `description`. It also implements exactly one of two checking traits, and its `RULES` entry (`RuleEntry::Whole` or `RuleEntry::Node`) names which.

Rules that need state across nodes (scope stacks, statement sequences, the scope tree's symbols) implement `WholeRule`, whose `fn check(&self, context: &LintContext<'_>) -> Vec<Finding>` sees the whole document at once. A rule that checks each block's statement list reads `context.blocks` instead of walking the tree, and any other walk goes through `Visitor`. The `LintContext` bundles the block, every block in pre-order, each function's block, the locals declared without a value, the flow facts about locals computed on first use, the semantic analysis, the source text, the comment array, and the resolved config.

Rules whose logic is node-local implement `NodeRule` instead: per-node hooks (`on_stmt`, `on_expr`, `on_table`, which gets every table constructor including `f{...}` arguments, and `on_call`, which gets every call in either position and marks which) that push onto a `Vec<Finding>`, and a `NodeTypeSet` of the `NodeType`s they act on. The `bus` makes one pass over the node table and calls each node rule only for the node types it declared, so every node rule shares a single traversal, and a node rule whose types don't occur in a file is skipped. In debug builds the driver re-runs every node rule on every node and asserts the results match, which catches a `node_types` set that misses a type.

A `Finding` holds only what differs per report: the message, span, optional help, and optional fix. The driver turns it into a public `LintDiagnostic` by adding the reporting rule's name and category and the severity the config resolves for it, so a rule can't report under another rule's name or set its own severity.

### Auto-fix

A rule with an always-safe transformation attaches a `Fix` to its finding. `Fix` holds a description and a list of `TextEdit`s, each a span and its replacement. `apply_fixes` sorts the edits, skips any that overlap an edit already applied, applies the rest in a single pass, and returns the original source if the result doesn't parse. `apply_fixes_fixpoint` re-lints and repeats until no fixes remain. Fixes that are sometimes wrong aren't shipped. There is no unsafe-fix tier.

### Module layout

| File | Purpose |
|------|---------|
| `lib.rs` | Crate root: the module list and the public re-exports |
| `driver.rs` | `lint()` and the other entry points, and the rule driver |
| `rule.rs` | `Rule`, `WholeRule`, and `NodeRule` traits, the `impl_rule!` macro, `CallPosition`, `LintContext` |
| `rules.rs` | `RULES` registry, `rules()`, and the registered rule names |
| `bus.rs` | The single-pass dispatch that drives every `NodeRule` in one shared walk |
| `nodes.rs` | Flat pre-order node table (`Nodes::collect`) of borrowed `NodeKind` views that the bus walks, with the set of node types present, the pre-order block table whole rules read, the block of the chunk and each function, the `UninitializedLocals` built in the same walk, and `adjacent_stmts` for the rules that match neighboring statements |
| `node_type.rs` | `NodeType`, one discriminant per statement, last-statement, and expression variant, and the `NodeTypeSet` bitset node rules declare |
| `cfg.rs` | `FlowGraph`, the control-flow graph of a block that rules ask how it ends and what can run, and the decision points `cyclomatic_complexity` counts |
| `local_flow.rs` | `LocalFlow`, what each function's control flow says about its locals, shared by the rules that follow a local's value and computed once per chunk |
| `diagnostic.rs` | `LintDiagnostic`, the `Finding` rules report, `Severity` and `Category` from `luck_core`, and the `line_column` positions that messages quote |
| `suppression.rs` | `-- luck:` directive parsing, shared with `invalid_lint_filter`, and suppression |
| `fix.rs` | `Fix` and `TextEdit`, and auto-fix application |
| `format_pattern.rs` | Validators for `string.format`, pattern, `string.pack`, `gsub` replacement, and `os.date` literals |
| `call.rs` | Whether a call still names the host's global, a `root.field(...)` callee's parts, plus positional arguments, argument count, and raw string-argument text, shared by the rules that match calls |
| `number.rs` | Values and digits of number literals, shared by `for_range`, `reversed_for_loop`, `manual_table_clone`, `table_operations`, `compare_nan`, `divide_by_zero`, `integer_parsing`, and the Roblox rules |
| `function_node.rs` | `FunctionNode`, one view of function statements and function expressions, for the node rules that check every function |
| `roblox.rs` | Shared helpers for the Roblox `<Global>.new(...)` constructor rules |
| `suggest.rs` | Levenshtein distance and the nearest-candidate search behind "did you mean" suggestions |
| `underscore_fix.rs` | The `_name` rename that `unused_variable`, `unused_argument`, and `unused_loop_variable` share, and when it's refused |
| `uninitialized.rs` | Locals declared without a value and field writes through a bare name, shared by the rules that track a local before its first assignment |
| `rules/*.rs` | Individual rule implementations |

### Testing

Each rule file carries its own `#[cfg(test)] mod tests` (positive cases named `flags_*`, negative cases `ignores_*`) built through `fixtures::run_rule` and its variants, which run the rule through the same analysis and dispatch as the driver. Fix tests apply the edits with `fixtures::fix_and_reparse`, which uses the same edit application as `--fix` and asserts the result re-parses. The `tests/it` binary drives the public API: config resolution (`config.rs`), directives (`directives.rs`), auto-fix (`fixes.rs`), rules firing end to end (`rules.rs`), and the shared `tests/fixtures/idiomatic/` corpus staying lint-clean (`idiomatic.rs`).
