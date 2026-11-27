# luck_linter

A Luau linter with scope-aware rules, auto-fixes, and suppression comments.

Rules fall into four categories: correctness, suspicious, style, and
performance. Only correctness rules run by default. Rules see every scope and
reference and each function's control flow, and they check globals against a
standard library from `luck_stdlib`, which can include Roblox's API or a host's
own definitions.

A `Linter` compiles its `LintConfig` once, then lints any number of files.
Fixes come in two kinds. A `FixKind::Fix` keeps the code's behavior and is safe
to apply automatically, while a `FixKind::Suggestion` might change it, so it
should only be applied by hand.

A `-- luck: allow(rule)` comment silences a rule for the next statement, and
adding `start` and `end` covers a region.

## Usage

This example reports an undefined global, then fixes an unused local:

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

let parse = luck_parser::parse(&allocator, b"local unused = 1");
let fixed = linter.fix(&allocator, parse, stdlib, FixKind::all(), None);
assert_eq!(fixed.map(|fixed| fixed.parse.source), Ok(&b"local _unused = 1"[..]));
```

## Configuration

`LintConfig` deserializes from the `lint` section of a `luck.json`:

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

* `Linter`, which lints a parse with `run` and applies fixes with `fix` until
  none are left. `analyze` builds an `Analysis`, which `run_analyzed` lints and
  can stop early when cancelled.
* `LintConfig`, with `CategoryLevels`, `RuleSettings`, and `RuleLevel`, which
  pick the rules that run and their severity.
* `RuleId`, one rule, and `RuleId::ALL`, every rule.
* `Category`, a rule's category.
* `Message`, one diagnostic with its fixes.
* `Fix`, a set of `TextEdit`s that apply together, and `FixKind`, which says
  whether it's a fix or a suggestion.
* `Fixed` and `FixError`, what `fix` returns.
* `non_overlapping_edits`, the edits from a set of messages that can apply at
  once.
* `RuleTimings` and `RuleTiming`, how long each rule took.

## Cargo features

* `serde` - Derives `Deserialize` and `Serialize` for `LintConfig`, in the
  shape a `luck.json` config gives it.
* `schemars` - Derives `JsonSchema` for `LintConfig`. Enables `serde`.
