# luck_codegen

Prints a Lua or Luau syntax tree as the shortest source that reads back as the same tree.

## Overview

`compact` walks the tree once. It writes no comments, a space only where two pieces would otherwise lex as one token, and a `;` only before a statement that starts with `(` and would otherwise call the one before. The tree keeps only the parentheses that change what they hold, so it writes those and, as `oxc_codegen` does, derives every other pair from where each expression goes, with `luck_ast`'s `Expression::needs_parens`: the fewest that read back as the same tree. It spells each literal the shortest way its value reads back in the chunk's version, a float as a float on Lua 5.3+ and a string in whichever quote or long bracket costs least, or as the tree spells it when that's no longer, unchecked. It never reads source text, so a tree built in code prints as well as a parsed one. It leaves the tree as it is. Folding, renaming, and dropping the parentheses the tree keeps where they change nothing are `luck_minifier`'s work.

It recurses once per level of nesting, so it takes a tree within `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree built some other way with `luck_ast::block_nesting_depth` first.

## Usage

- `compact(&chunk)` returns the source of a tree, read in the version its `Chunk` records.
- `compact_with_capacity(&chunk, capacity)` reserves the output up front. A parsed tree's source length is a good hint.

```rust
use luck_allocator::Allocator;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, "local x = ( a + b ) * ( 0x10 )", LuaVersion::Lua54);
assert_eq!(luck_codegen::compact(&parsed.chunk), "local x=(a+b)*16");
```
