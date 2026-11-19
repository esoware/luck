# luck_formatter

The Prettier-style formatter for Lua 5.1 to 5.5 and Luau.

## Overview

The formatter writes the tree into one flat stream of layout elements in the caller's arena. The elements are text, line breaks, indents, and groups, and each text borrows from the tree, the source, or the arena. A printer then walks the stream once. A group prints on one line if it fits the line width, along with whatever follows it on that line, and breaks if it doesn't.

For a parse, the formatter takes comments from the parse's comment list and places each at the nearest node boundary: before the node it leads, after the node it trails on its line, or before a container's closer, as `oxc_formatter` does. A comment that ends its line in the source ends it in the output. It keeps blank lines between statements, and copies `-- luck: format off` to `-- luck: format on` regions and statements after `-- luck: ignore` as written. A tree built in code takes its comments from `SyntheticComment`s instead. The output parses, and formatting it again changes nothing.

## Features

- `FormatOptions` sets the line width, indent style and width, quote style, hex digit case, call parentheses, collapsing of simple `if` statements, line endings, blank lines at the edges of blocks, opt-in `require` sorting, a space after function names, and the magic trailing comma.
- The `serde` feature derives `Deserialize` for the option enums, the shape `luck.json` gives them, and `schemars` derives `JsonSchema` too.

## Usage

- `format(allocator, source, version, &options)` parses and formats, returning a `FormatResult` with the output and any parse errors.
- `format_parsed` formats a parse the caller holds, and `format_range` formats only the statements a byte range overlaps and copies the rest.
- `format_chunk(allocator, &chunk, comments, &options)` formats a tree with no source, such as one built with `luck_ast::Synth`, with `Comments::synthetic` or `Comments::none`. It fails only on a tree nested past `luck_ast::MAX_NESTING_DEPTH`.
- Every entry point that takes a tree reads its dialect from the tree's `Chunk`.

```rust
use luck_allocator::Allocator;
use luck_formatter::FormatOptions;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let result =
    luck_formatter::format(&allocator, "local x=1", LuaVersion::Lua54, &FormatOptions::default());
assert!(result.errors.is_empty());
assert_eq!(result.output, "local x = 1\n");
```
