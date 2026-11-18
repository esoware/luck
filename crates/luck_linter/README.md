# luck_linter

The Lua and Luau linter, with scope-aware rules, auto-fixes, and suppression comments.

## Overview

`analyze` reads a parse's lint directives, runs `luck_semantic` over it, with the configured extra globals defined, and builds the linter's table of the tree's nodes in the same walk. The rules then read all three. Rules that look at one node at a time run together in one pass over the node table. Rules that need the whole document, such as scope or statement order, run alongside that pass, on rayon's pool for a large file. Each finding becomes a `LintDiagnostic` with its rule's name, category, and severity, unless a directive covering it drops it or sets its severity. A parse with errors gives one `parse_error` diagnostic per error and runs no rules.

A diagnostic may carry a `Fix`, a set of text edits applied together, of one `FixKind`. A `Fix` keeps what the code does, so `--fix` and the editor's fix-all apply it. A `Suggestion` can change what the code does, or hides a problem rather than solving it, as renaming an unused variable to `_x` does, so only the user applies it, as an editor quick fix or with `--fix-suggestions`. Each rule declares the kinds its fixes come in. A host names the kinds it applies, and a rule builds a fix only of those, so a plain lint builds none. Every fix but the one that removes an unused directive passes one check before it's kept: an edit that replaces a comment must carry it, or the fix is dropped.

## Features

- Rules fall into four categories: correctness, suspicious, style, and performance. Only correctness rules run by default. `LintConfig` sets a level per category or per rule, a rule's options, and extra globals.
- A comment such as `-- luck: allow(unused_variable)` silences a rule for the statement it trails, or else the next one, `deny` and `warn` make what it reports an error or a warning, `start` and `end` after it mark a region, and `-- #luck: allow(...)` covers the whole file. A `deny` or `warn` also runs a rule config turns off, over what it covers. `unused_lint_filter` reports an `allow` that silences nothing, with a fix that removes it, and `invalid_lint_filter` a malformed directive or a name no rule has.
- The `serde` feature derives `Deserialize` and `Serialize` for `LintConfig`, in the shape of `luck.json`'s `lint` section, and `schemars` derives `JsonSchema` too. Deserializing rejects a name no rule has, and suggests a close one.

## Usage

- `lint(allocator, source, target, config, fixes)` parses and lints, building the fixes of the kinds `fixes` holds.
- `lint_parsed` lints a parse the caller holds. A host that also keeps the analysis, as the language server does, calls `analyze` and then `lint_analyzed`, which asks the host between rules whether to stop, so a lint of a version the user has already edited stops early.
- `fix` lints and applies the fixes of the kinds asked for, round after round, until none is left. Of two overlapping fixes a round applies one, and the next round lints the result again. It returns the fixed source's parse and diagnostics, or an error when a round's fixes break the syntax or fixes are still coming after 10 rounds, so a caller writes back every fix or none.
- `non_overlapping_edits` picks the edits of one round, which the language server's fix-all sends.
- `rules()` lists every rule as a `&dyn Rule` with its name, category, default severity, description, and fix kinds, and `rule_named` looks one up by name, with a message that suggests a close name when none matches.

```rust
use luck_allocator::Allocator;
use luck_core::LuaTarget;
use luck_linter::{FixKind, LintConfig};

let allocator = Allocator::new();
let config = LintConfig::default();
let source = "print(undefined_name)";
let diags = luck_linter::lint(&allocator, source, LuaTarget::Lua54, &config, FixKind::empty());
assert_eq!(diags[0].rule, "undefined_variable");
```
