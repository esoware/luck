# luck_lexer

The single-pass lexer for Lua 5.1 to 5.5 and Luau.

## Overview

Source text goes in and two streams come out: the tokens, and the comments between them, which stay out of the syntax tree. A token holds no text, only its kind, span, and flags, so `Token::text` slices it from the source. The lexer recognizes a token that only some versions have, such as floor division, a hex float, or Luau's interpolated strings, only for those versions. It records each error and keeps going, so one pass reports them all.

## Usage

- `lex(source, version)` lexes a whole source into a `LexResult`: every token, ending in `Eof`, the comments, the errors, and the spans of strings and comments left unterminated.
- `Lexer` hands out one token at a time with `next_token`, as the parser pulls them, and `finish` returns the comments and errors once the source is drained.
- `LexError` is `luck_token::SourceError`. The lexer refuses a source over 4 GiB with one.

```rust
use luck_token::{LuaVersion, TokenKind};

let source = "local x = 42";
let result = luck_lexer::lex(source, LuaVersion::Lua54);
assert!(result.errors.is_empty());
let name = result.tokens[1];
assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, "x"));
```
