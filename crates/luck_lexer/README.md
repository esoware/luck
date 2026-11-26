# luck_lexer

A single-pass Luau lexer that scans with SIMD on AVX2 and Neon.

## Overview

Turns a source into a flat list of tokens and a separate list of comments. AVX2 is used on `x86_64` CPUs that have it.

Strings and comments can contain any byte. Invalid UTF-8 anywhere else is an error.

## Usage

```rust
use luck_token::TokenKind;

let source = b"local x = 42";
let result = luck_lexer::lex(source);
assert!(result.errors.is_empty());
let name = result.tokens[1];
assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
```

## API

- `lex` lexes a whole source.
- `LexResult` holds the tokens, which end in `Eof`, the comments, and the errors.
- `Lexer` produces one token at a time, for the parser.
