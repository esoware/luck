# luck_parser

A hand-written, error-recovering parser for Luau, including its full type
syntax.

`parse` reads a source into a `luck_ast` tree in an arena you pass in, using
Pratt parsing for expressions and recursive descent for statements. When it
hits a syntax error, it recovers and keeps going, so a single parse reports
every error in the file. As in Luau, strings and comments may hold any byte.

No tree it returns nests deeper than `luck_ast::MAX_NESTING_DEPTH`, so anything
that walks the tree recursively is safe on a thread with
`luck_ast::MIN_STACK_BYTES` of stack.

## Usage

This example parses one statement, then shows the error a truncated function
gets:

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

* `parse`, which parses code.
* `parse_definitions`, which parses a definition file, where Luau's `declare`
  statements are allowed.
* `parse_with_tokens` and `parse_definitions_with_tokens`, which do the same
  and also keep the token list, as a formatter needs.
* `ParseResult`, the tree with its comments, tokens, and errors, and node counts
  for sizing later passes.
* `Groupings`, the parentheses the tree left out, which a formatter uses to keep
  the source's grouping.
