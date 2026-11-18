# luck_linter

The Lua and Luau linter, with scope-aware rules, auto-fixes, and suppression comments.

## Overview

`analyze` runs `luck_semantic` over a parse, with the configured extra globals defined, and builds the linter's table of the tree's nodes in the same walk. The rules then read both. Rules that look at one node at a time share a single dispatch over the node table, and rules that need the whole document, such as scope or statement order, run beside it, on rayon's pool for a large file. Each finding becomes a `LintDiagnostic` carrying its rule's name, category, and severity. A parse with errors gives one `parse_error` diagnostic per error and runs no rules.

A diagnostic may carry a `Fix`, a set of text edits. `apply_fixes` applies the edits that don't overlap and returns the fixed source's parse, or `None` when it doesn't parse, so a broken fix never lands.

## Features

- Rules fall into four categories: correctness, suspicious, style, and performance. Only correctness rules run by default. `LintConfig` sets a level per category or per rule, a rule's options, and extra globals.
- A comment such as `-- luck: allow(unused_variable)` silences a rule for one statement, `deny` and `warn` change its severity, `start` and `end` after it mark a region, and `-- #luck: allow(...)` covers the whole file.
- The `serde` feature derives `Deserialize` and `Serialize` for `LintConfig`, in the shape of `luck.json`'s `lint` section, and `schemars` derives `JsonSchema` too.

## Public surface

- `lint(allocator, source, target, config)` parses and lints.
- `lint_parsed` lints a parse the caller holds. A host that also keeps the analysis, as the language server does, calls `analyze` and then `lint_analyzed`.
- `apply_fixes` fixes one round, and `apply_fixes_fixpoint` lints and fixes until no fix is left.
- `rules()` lists every rule as a `&dyn Rule` with its name, category, default severity, and description, and `unknown_rule_names` checks a config's rule names.

```rust
use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::LintConfig;

let allocator = Allocator::new();
let config = LintConfig::default();
let diags = luck_linter::lint(&allocator, "print(undefined_name)", LuaTarget::Lua54, &config);
assert_eq!(diags[0].rule, "undefined_variable");
```
