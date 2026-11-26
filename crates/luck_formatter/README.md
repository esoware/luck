# luck_formatter

A Prettier-style formatter for Luau that fits code to a line width and handles the full type syntax.

## Overview

`format(format(x)) == format(x)`, and the output always parses. Tests check both.

Comments stay with the nearest node, and blank lines between statements are kept. Code between `-- luck: format off` and `-- luck: format on` is left alone, and `-- luck: ignore` skips the next statement.

`format_range` formats a selection. `format_chunk` formats a tree with no source, such as one built with `luck_ast::Synth`.

## Usage

```rust
use luck_allocator::Allocator;
use luck_formatter::FormatOptions;

let allocator = Allocator::new();
let result = luck_formatter::format(&allocator, b"local x=1", &FormatOptions::default());
assert!(result.errors.is_empty());
assert_eq!(result.output, b"local x = 1\n");
```

## API

- `format` parses and formats a source.
- `format_parsed` formats a parse you already have, from `luck_parser::parse_with_tokens`.
- `format_range` formats the statements a byte range overlaps, and copies the rest unchanged.
- `format_chunk` formats a tree, with its comments passed as `Comments`.
- `FormatResult` holds the output and the parse errors.
- `Comments` and `SyntheticComment` attach comments to a tree built in code.
- `FormatOptions` holds every option, with defaults filled in.
- `FormatConfig` is the `format` section of `luck.json`, where each option is optional.
- `IndentStyle`, `QuoteStyle`, `HexCase`, `CallParentheses`, `CollapseSimpleStatement`, `LineEndings`, `BlockNewlineGaps`, and `SpaceAfterFunction` are option values.

## Cargo features

- `serde` derives `Deserialize` for `FormatConfig` and the option enums, in the shape `luck.json` uses.
- `schemars` derives `JsonSchema` too, and enables `serde`.
