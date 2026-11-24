# luck_linter

The Lua and Luau linter, with scope-aware rules, auto-fixes, and suppression comments.

## Overview

A `Linter` compiles a `LintConfig` once: each rule is built from its options, and its severity resolved, into a table every file it lints reads by `RuleId`, as `oxc_linter::Linter` reads its config store's resolved state. Its `analyze` reads a parse's lint directives and runs `luck_stdlib::analyze` over it against a standard library, built in or with a project's std definitions layered over it, with the configured extra globals defined, which builds the chunk's `luck_semantic::Semantic`: its scoping, its `AstNodes` table, every node in source preorder with its parent and scope, and its `require` sites. The rules read both. They run as oxlint's do: a rule's `run` takes one node of the types it lists, and every such rule runs in one pass over the node table, or its `run_once` reads the file once, as the scoping or the directives. A rule finds a node's context through its parents, such as whether a chain is a call statement or what an assignment writes. Each function's control-flow graph, and the stdlib entries a chain's calls reach, are built once, the first time a rule asks, and every rule shares them. A rule that asks what an expression evaluates to, such as whether a condition is always truthy or a divisor always zero, asks `luck_value`, as the minifier does, with a context that knows a local from a global. A file is linted on the thread that asks, and a host lints files in parallel. Each finding becomes a `Message`: a `luck_diagnostics::Diagnostic` coded with its rule's name, at the severity config or a directive gives it, with a link to the rule's source and any labels, help, and editor tag the rule gives, and the fix, if any. A directive covering a finding drops it or sets its severity. A parse with errors gives its E008 errors as messages and runs no rules, so nothing can turn one off, and so does a chunk Lua refuses to compile for an error semantic analysis finds, such as a write to a `<const>` local, a `goto` no label takes, or more locals active in a function than Lua compiles.

A diagnostic may carry a `Fix`, a set of text edits applied together, of one `FixKind`. A `Fix` keeps what the code does, so `--fix` and the editor's fix-all apply it. A `Suggestion` can change what the code does, or hides a problem rather than solving it, as renaming an unused variable to `_x` does, so only the user applies it, as an editor quick fix or with `--fix-suggestions`. Each rule declares the kinds its fixes come in. A host names the kinds it applies, and a rule builds a fix only of those, so a plain lint builds none. Every fix but the one that removes an unused directive passes one check before it's kept: an edit that replaces a comment must carry it, or the fix is dropped.

## Features

- Each rule is declared once, in its own module, with `declare_lint_rule!`, as oxlint declares its rules with `declare_oxc_lint!`: its category, default severity, description, fix kinds, and options type. Its name is its module's, and `declare_all_lint_rules!` lists every module once, which gives each rule its `RuleId`, the rule enum the linter dispatches through, and the typed table of every rule's options.
- Rules fall into four categories: correctness, suspicious, style, and performance. Only correctness rules run by default. `LintConfig` sets a level per category or per rule, a rule's options, and extra globals.
- A comment such as `-- luck: allow(unused_variable)` silences a rule for the statement it trails, or else the next one, `deny` and `warn` make what it reports an error or a warning, `start` and `end` after it mark a region, and `-- #luck: allow(...)` covers the whole file. A `deny` or `warn` also runs a rule config turns off, over what it covers. `unused_lint_filter` reports an `allow` that silences nothing, with a fix that removes it, and `invalid_lint_filter` a malformed directive or a name no rule has.
- The `serde` feature derives `Deserialize` and `Serialize` for `LintConfig`, in the shape of `luck.json`'s `lint` section, and `schemars` derives `JsonSchema` too, which names every rule. Deserializing rejects a name no rule has, and suggests a close one, and options on a rule that takes none.

## Usage

- `Linter::new(config)` compiles the settings once, and `run(parse, stdlib, fixes)` lints a parse with them, building the fixes of the kinds `fixes` holds. A host that also keeps the analysis, as the language server does, calls `analyze` and then `run_analyzed`, which asks the host between rules whether to stop, so a lint of a version the user has already edited stops early.
- A source is bytes, as Lua reads it, and so is a fix's replacement, which keeps the bytes of the code it copies.
- `Linter::fix` lints and applies the fixes of the kinds asked for, round after round, until none is left. Of two overlapping fixes a round applies one, and the next round lints the result again. It returns the fixed source's parse and diagnostics, or an error when a round's fixes break the syntax or fixes are still coming after 10 rounds, so a caller writes back every fix or none.
- `non_overlapping_edits` picks the edits of one round, which the language server's fix-all sends.
- `RuleId::ALL` lists every rule, with its name, category, default severity, description, fix kinds, and docs link, the rule's source on GitHub at this release. `RuleId::from_name` looks one up by name, and parsing a `RuleId` fails with a message that suggests a close name when none matches.

```rust
use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter};
use luck_stdlib::library_for;
use luck_token::LuaTarget;

let allocator = Allocator::new();
let linter = Linter::new(LintConfig::default());
let parse = luck_parser::parse(&allocator, b"print(undefined_name)", LuaTarget::Lua54.version());
let diags = linter.run(&parse, library_for(LuaTarget::Lua54), FixKind::empty());
assert_eq!(diags[0].code, "undefined_variable");
```
