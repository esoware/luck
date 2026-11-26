# luck_parser

A hand-written Luau parser with error recovery, which builds a `luck_ast` tree.

## Overview

The tree goes in an arena you pass in. Expressions use Pratt parsing and statements use recursive descent. It recovers from syntax errors, so one parse reports all of them.

`parse_definitions` reads definition files, which allow Luau's `declare` statements. `parse_with_tokens` also keeps the tokens, which the formatter needs.

No tree it returns nests deeper than `luck_ast::MAX_NESTING_DEPTH`. Strings and comments can contain any byte.

## Usage

```rust
use luck_allocator::Allocator;

let allocator = Allocator::new();
let result = luck_parser::parse(&allocator, b"local x = 1 + 2");
assert!(result.errors.is_empty());
assert_eq!(result.chunk.stmts.len(), 1);

let broken = luck_parser::parse(&allocator, b"local function f(");
assert_eq!(broken.errors[0].code, "E008");
```

## API

- `parse` parses code.
- `parse_with_tokens` parses code and keeps the tokens.
- `parse_definitions` parses a definition file.
- `parse_definitions_with_tokens` parses a definition file and keeps the tokens.
- `ParseResult` holds the tree, comments, tokens, errors, and node counts.
- `Groupings` records the parentheses the tree drops.
