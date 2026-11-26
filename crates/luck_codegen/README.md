# luck_codegen

A compact code printer for Luau syntax trees, used by the minifier and the bundler.

## Overview

Writes the fewest characters that parse back to the same tree. It never reads the source, so trees built in code print too.

Comments are dropped. Literals print as written, or in their shortest form with `LiteralSpelling::Shortest`.

It takes trees no deeper than `luck_ast::MAX_NESTING_DEPTH`, which every parsed tree is.

## Usage

```rust
use luck_allocator::Allocator;
use luck_codegen::LiteralSpelling;

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"local x = ( a + b ) * ( c )");
let output = luck_codegen::compact(&parsed.chunk, LiteralSpelling::AsWritten);
assert_eq!(output, b"local x=(a+b)*c");
```

## API

- `compact` prints a chunk into a new buffer.
- `compact_into` prints into a buffer you pass, reusing its memory.
- `LiteralSpelling` is `AsWritten` or `Shortest`.
