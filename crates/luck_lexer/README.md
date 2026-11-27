# luck_lexer

A single-pass Luau lexer, with SIMD scanning on AVX2 and Neon.

The lexer turns a source into a flat list of tokens, ending in `Eof`, and a
separate list of comments. Tokens don't hold any text, so the output is only
kinds, spans, and flags. On `x86_64` it uses AVX2 when the CPU has it, and on
aarch64 it always uses Neon.

Like Luau, it reads the source as bytes. A string or a comment may hold any
byte, and invalid UTF-8 anywhere else is reported as an error.

## Usage

This example lexes a local declaration and checks its second token:

```rust
use luck_token::TokenKind;

let source = b"local x = 42";
let result = luck_lexer::lex(source);
assert!(result.errors.is_empty());
let name = result.tokens[1];
assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
```

## API

* `lex`, which lexes a whole source into a `LexResult` of tokens, comments, and
  errors.
* `Lexer`, which hands out tokens one at a time, for a parser to pull from.
