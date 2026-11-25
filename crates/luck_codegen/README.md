# luck_codegen

Prints a Luau syntax tree as the shortest source that reads back as the same tree.

## Overview

`compact` walks the tree once, and each expression goes straight from where it's reached to the printer for its kind. It writes no comments, a space only where two pieces would otherwise lex as one token, and a `;` only before a statement that starts with `(` and would otherwise call the one before. Each piece leaves a class behind that says what the next one would merge with, and a keyword, a symbol, or an operator is a constant whose classes are too. A numeral reads on through `.` only before its first letter, which only a piece that starts with `.` asks about, so its text is read then and not before. The tree keeps only the parentheses that change what they hold, so it writes those and, as `oxc_codegen` does, derives every other pair from where each expression goes, with `luck_ast`'s `Slot`, which says of each kind of expression whether it needs them there: the fewest that read back as the same tree. It spells each literal as the tree spells it, unchecked, or with `LiteralSpelling::Shortest`, as the minifier asks, the shortest way its value reads back when that's shorter, a string in whichever quote or long bracket costs least. It never reads source text, so a tree built in code prints as well as a parsed one, its literals spelled from their values. It writes bytes, as a source is, which are UTF-8 unless a literal as the tree spells it isn't. It leaves the tree as it is. Folding, renaming, and dropping the parentheses the tree keeps where they change nothing are `luck_minifier`'s work.

It recurses once per level of nesting, so it takes a tree within `luck_ast::MAX_NESTING_DEPTH`, as `luck_parser` builds. Measure a tree built some other way with `luck_ast::block_nesting_depth` first.

## Usage

- `compact(&chunk, spelling)` returns the source of a tree, as bytes, with its literals as written or, for `LiteralSpelling::Shortest`, as short as they go. It reserves as much as the tree spans, which for a parsed tree is about its source's length.
- `compact_into(&chunk, spelling, output)` writes after the bytes `output` holds, in its buffer, so a header such as a shebang costs no copy of the code, and starts with the capacity `output` reserves. A parsed tree's source length, its comments aside, is a good hint.

```rust
use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"local x = ( a + b ) * ( 0x10 )");
assert_eq!(luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten), b"local x=(a+b)*0x10");
assert_eq!(luck_codegen::compact(&parsed.chunk, LiteralSpelling::Shortest), b"local x=(a+b)*16");
```
