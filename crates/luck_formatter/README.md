# luck_formatter

A Prettier-style code formatter for Luau, including its full type syntax.

The formatter lays code out to fit a line width, and breaks a group across
lines only when it doesn't fit on one. Comments stay attached to the nearest
node, and blank lines between statements are kept. Formatting is idempotent and
its output always parses, which the tests check.

Code between `-- luck: format off` and `-- luck: format on` is copied as
written, and `-- luck: ignore` does the same for the next statement. A string
or comment that isn't valid UTF-8 is copied as written too.

## Usage

This example formats a one-line local with the default options:

```rust
use luck_allocator::Allocator;
use luck_formatter::FormatOptions;

let allocator = Allocator::new();
let result = luck_formatter::format(&allocator, b"local x=1", &FormatOptions::default());
assert!(result.errors.is_empty());
assert_eq!(result.output, b"local x = 1\n");
```

`format` parses the source itself. If you already have a parse from
`luck_parser::parse_with_tokens`, `format_parsed` skips that step, and
`format_range` formats only the statements a byte range overlaps, which is
what an editor's format selection needs.

## API

* `format`, `format_parsed`, and `format_range`, which return a `FormatResult`
  with the output and any parse errors.
* `format_chunk`, which formats a tree that has no source, such as one built
  with `luck_ast::Synth`, taking its comments as `Comments` built from
  `SyntheticComment`s.
* `FormatOptions`, every option with its default filled in.
* `FormatConfig`, the same options with each one optional, as a config file
  gives them, which layers over another config or the defaults.
* `IndentStyle`, `QuoteStyle`, `HexCase`, `CallParentheses`,
  `CollapseSimpleStatement`, `LineEndings`, `BlockNewlineGaps`, and
  `SpaceAfterFunction`, the values of the enum options.

## Cargo features

* `serde` - Derives `Deserialize` for `FormatConfig` and the option enums, in
  the shape a `luck.json` config gives them.
* `schemars` - Derives `JsonSchema` for the same types. Enables `serde`.
