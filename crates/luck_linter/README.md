# luck_linter

A Luau linter with scope-aware rules, auto-fixes, and inline suppression comments.

## Overview

Rules come in four categories: correctness, suspicious, style, and performance. Only correctness runs by default. A `Linter` compiles its `LintConfig` once, then lints any number of files.

Rules see scopes, references, and control flow, and get the known globals from `luck_stdlib`.

A fix is either a `FixKind::Fix`, which keeps behavior and is what `--fix` applies, or a `FixKind::Suggestion`, which might not, so only the user applies it.

`-- luck: allow(rule)` silences a rule for the next statement. Add `start` and `end` to cover a region.

## Usage

```rust
use luck_allocator::Allocator;
use luck_linter::{FixKind, LintConfig, Linter};
use luck_stdlib::luau_library;

let allocator = Allocator::new();
let linter = Linter::new(LintConfig::default());
let stdlib = luau_library();

let parse = luck_parser::parse(&allocator, b"print(undefined_name)");
let diags = linter.run(&parse, stdlib, FixKind::empty(), None);
assert_eq!(diags[0].code, "undefined_variable");

let fixed = linter.fix(&allocator, b"local unused = 1", stdlib, FixKind::all(), None);
assert_eq!(fixed.map(|fixed| fixed.parse.source), Ok(&b"local _unused = 1"[..]));
```

## Configuration

`LintConfig` is the `lint` section of `luck.json`:

```json
{
  "lint": {
    "categories": { "suspicious": "warn" },
    "rules": { "shadowing": "off" },
    "extra_globals": ["describe", "expect"]
  }
}
```

## API

- `Linter` lints a parse with `run`, or an `Analysis` with `run_analyzed`, and fixes a source with `fix`.
- `Analysis` is a parse with its semantic analysis and lint directives, from `Linter::analyze`.
- `LintConfig`, `CategoryLevels`, `RuleSettings`, and `RuleLevel` pick which rules run and at what severity.
- `RuleId` names a rule, and `RuleId::ALL` lists them all.
- `Category` is a rule's category.
- `Message` is one diagnostic with its fixes.
- `Fix` is a set of `TextEdit`s, and `FixKind` says whether it's a fix or a suggestion.
- `Fixed` and `FixError` are what `fix` returns.
- `non_overlapping_edits` picks the edits from a set of messages that can apply together.
- `RuleTimings` and `RuleTiming` record how long each rule took.

## Cargo features

- `serde` derives `Deserialize` and `Serialize` for `LintConfig`, in the shape `luck.json` uses.
- `schemars` derives `JsonSchema` too, and enables `serde`.
