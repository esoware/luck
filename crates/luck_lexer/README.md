# luck_lexer

The single-pass Luau lexer, with SIMD on x86_64 CPUs with AVX2 and on aarch64.

## Overview

A source goes in, as bytes, which Luau reads it as, so a string or a comment may hold any byte, and two streams come out: the tokens, and the comments between them, which stay out of the syntax tree. Each comment carries what it tells a tool, read once from its text: prose, a hot comment such as `--!strict`, or a `luck:` directive. A token holds no text, only its kind, span, and flags, so `Token::text` slices its bytes from the source. A contextual keyword, such as `type` or `continue`, has a kind of its own, which the parser reads as a name wherever its keyword reading doesn't apply. It checks every escape with the one escape reader in `luck_token`, the parser's too, records each error and keeps going, so one pass reports them all, and notes each escape of a character Luau defines no escape for, which it reads as that character.

On a CPU with AVX2 or Neon, which `luck_simd` detects at runtime, the lexer classifies 64 bytes at a time into a bit per byte for each class it cares about, such as whitespace, name bytes, punctuation, and quotes, works out once per window where its tokens start and end, and reads the names, keywords, decimal integers, hexadecimal integers, decimal fractions, punctuation and the operators it joins into, and quoted strings without escapes among them off those bits. Any other token, such as a comment, a long bracket, a string with an escape, an interpolated string, `~=`, or a malformed one, it lexes a byte at a time, as it lexes every token on any other CPU, and the window stays classified past it, so the tokens after it come off the same bits. Both paths read every source alike, which a test checks over every fixture and thousands of generated sources.

## Usage

- `lex(source)` lexes a whole source into a `LexResult`: every token, ending in `Eof`, the comments, the errors, and the spans of the lenient escapes. Each batch goes straight into the vector it returns. A string token left unterminated says so, and so does the `Eof` after a block comment left unterminated. A string token says too whether it holds an escape, or in long brackets a CR, so the parser decodes only the strings that need it.
- `Lexer` hands out one token at a time with `next_token`, from a batch it lexes ahead into a buffer that stays in the cache, and once the source is drained, `finish` takes back the tokens the caller kept and returns them in a `LexResult` with the rest. A caller that reads every token, as the parser does, can take each with `has_buffered` and `take_buffered`, which call nothing, look at the one after it in place with `peek_buffered`, and lex the next batch with `refill` out of line.
- Each error is an E008 `luck_diagnostics::Diagnostic`. The lexer refuses a source over 4 GiB with one.

```rust
use luck_token::TokenKind;

let source = b"local x = 42";
let result = luck_lexer::lex(source);
assert!(result.errors.is_empty());
let name = result.tokens[1];
assert_eq!((name.kind, name.text(source)), (TokenKind::Identifier, &b"x"[..]));
```
