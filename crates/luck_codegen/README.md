# luck_codegen

Prints a Luau syntax tree as the shortest source that parses back to the same
tree.

`compact` writes no comments, puts a space only where two tokens would
otherwise run together, and adds a `;` only where the next statement would
otherwise read as a call. It keeps the parentheses the tree holds and adds the
fewest others needed for the output to parse back the same way.

Literals print as written, or with `LiteralSpelling::Shortest`, in the shortest
form of their value. The printer never reads source text, so a tree built in
code prints too. It recurses once per level of nesting, so give it trees within
`luck_ast::MAX_NESTING_DEPTH`, as every parsed tree is.

## Usage

This example parses an expression with redundant parentheses and prints it back
compactly:

```rust
use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"local x = ( a + b ) * ( c )");
let output = luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten);
assert_eq!(output, b"local x=(a+b)*c");
```

## API

* `compact`, which prints a chunk into a new buffer.
* `compact_into`, which prints into a buffer you pass in, reusing its memory.
* `LiteralSpelling`, either `AsWritten` or `Shortest`.
