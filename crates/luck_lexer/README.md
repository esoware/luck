# luck_lexer

The single-pass lexer for Lua 5.1 to 5.5 and Luau.

## Overview

A source goes in, as bytes, which Lua reads it as, so a string or a comment may hold any byte, and two streams come out: the tokens, and the comments between them, which stay out of the syntax tree. Each comment carries what it tells a tool, read once from its text: prose, a Luau hot comment, or a `luck:` directive. A token holds no text, only its kind, span, and flags, so `Token::text` slices its bytes from the source. The lexer recognizes a token that only some versions have, such as floor division, a hex float, or Luau's interpolated strings, only for those versions. It checks every escape with the one escape reader in `luck_token`, the parser's too, records each error and keeps going, so one pass reports them all, and notes each escape only Lua 5.1 and Luau take, which they read as the character it escapes.

## Usage

- `lex(source, version)` lexes a whole source into a `LexResult`: every token, ending in `Eof`, the comments, the errors, the spans of strings and comments left unterminated, and the spans of the lenient escapes.
- `Lexer` hands out one token at a time with `next_token`, as the parser pulls them, and `finish` returns the rest of a `LexResult` once the source is drained.
- Each error is an E008 `luck_diagnostics::Diagnostic`. The lexer refuses a source over 4 GiB with one.

```rust
use luck_token::{LuaVersion, TokenKind};

let source = b"local x = 42";
let result = luck_lexer::lex(source, LuaVersion::Lua54);
assert!(result.errors.is_empty());
let name = result.tokens[1];
assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
```
