# luck_codegen

Prints a Lua or Luau syntax tree as the shortest source that reads back as the same tree.

## Overview

`compact` walks the tree once. It writes no comments, a space only where two pieces would otherwise lex as one token, and a `;` only before a statement that starts with `(` and would otherwise call the one before. It never reads source text, so a tree built in code prints as well as a parsed one, and it prints each leaf's text as the tree holds it, unchecked. It changes nothing about the tree itself: dropping parentheses, folding, and renaming are `luck_minifier`'s work.

It recurses once per level of nesting, so it takes a tree within `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree built some other way with `luck_ast::block_nesting_depth` first.

## Usage

- `compact(&block)` returns the source.
- `compact_with_capacity(&block, capacity)` reserves the output up front. A parsed tree's source length is a good hint.

```rust
use luck_allocator::Allocator;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, "local x = ( 1 )", LuaVersion::Lua54);
assert_eq!(luck_codegen::compact(&parsed.block), "local x=(1)");
```
