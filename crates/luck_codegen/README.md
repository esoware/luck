# luck_codegen

Prints a Lua or Luau syntax tree as the shortest source that reads back as the same tree.

## Overview

`compact` walks the tree once. It writes no comments, a space only where two pieces would otherwise lex as one token, and a `;` only before a statement that starts with `(` and would otherwise call the one before. The tree keeps only the parentheses that change what they hold, so it writes those and, as `oxc_codegen` does, derives every other pair from where each expression goes, with `luck_ast`'s `Expression::needs_parens`: the fewest that read back as the same tree. It spells each literal as the tree spells it, unchecked, or with `LiteralSpelling::Shortest`, as the minifier asks, the shortest way its value reads back in the chunk's version when that's shorter, a float as a float on Lua 5.3+ and a string in whichever quote or long bracket costs least. It never reads source text, so a tree built in code prints as well as a parsed one, its literals spelled from their values. It writes bytes, as a source is, which are UTF-8 unless a literal as the tree spells it isn't. It leaves the tree as it is. Folding, renaming, and dropping the parentheses the tree keeps where they change nothing are `luck_minifier`'s work.

It recurses once per level of nesting, so it takes a tree within `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree built some other way with `luck_ast::block_nesting_depth` first.

## Usage

- `compact(&chunk, spelling)` returns the source of a tree, as bytes, read in the version its `Chunk` records, with its literals as written or, for `LiteralSpelling::Shortest`, as short as they go.
- `compact_with_capacity(&chunk, spelling, capacity)` reserves the output up front. A parsed tree's source length is a good hint.

```rust
use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"local x = ( a + b ) * ( 0x10 )", LuaVersion::Lua54);
assert_eq!(luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten), b"local x=(a+b)*0x10");
assert_eq!(luck_codegen::compact(&parsed.chunk, LiteralSpelling::Shortest), b"local x=(a+b)*16");
```
