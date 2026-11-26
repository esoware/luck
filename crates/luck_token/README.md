# luck_token

Tokens, spans, operators, and literal handling shared by every luck crate that reads or writes Luau.

## Overview

It depends on no other luck crate.

Luau reads sources as bytes, so strings and comments don't have to be UTF-8. Spans are `u32` offsets, which caps a source at 4 GiB.

A `Token` stores no text. Get it from the source with `Token::text`.

It also reads and writes number and string literals exactly as Luau does, and recognizes `--!` hot comments and `luck:` directives.

## Usage

```rust
use luck_token::{Span, Token, TokenKind};

assert_eq!(TokenKind::keyword("continue"), Some(TokenKind::Continue));
assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));

let source = b"`hello {name}`";
let begin = Token {
    span: Span::new(0, 8),
    kind: TokenKind::InterpolatedStringBegin,
    has_line_break_before: true,
    is_unterminated: false,
    has_escape: false,
};
assert_eq!(begin.text(source), b"hello ");
```

## API

- Spans: `Span`, `index_to_offset`, `offset_to_index`
- Tokens: `Token`, `TokenKind`
- Operators: `BinOp`, `UnOp`, `CompoundOp`, `Associativity`
- Numbers: `Numeral`, `NumberSpelling`, `read_numeral`, `spell_number`
- Strings: `StringValue`, `StringKind`, `StringSpelling`, `Escaped`, `EscapeError`
- String functions: `read_escape`, `decode_string`, `decode_interpolated_text`, `spell_string`, `spell_string_shorter_than`, `write_interpolated_text`, `write_long_string`
- Comments: `Comment`, `CommentKind`, `CommentPosition`
- Directives: `CommentContent`, `LintDirective`, `LintVerb`, `LintExtent`, `DirectiveError`, `DirectiveErrorKind`
- Lines: `LineIndex`, `line_breaks`, `has_line_break`, `ends_with_line_break`, `strip_line_break`, `is_luau_whitespace`
- Chunk headers: `has_shebang`, `leading_hot_comments`, `chunk_header`
- Limits: `MAX_ACTIVE_LOCALS`, `MAX_REGISTERS`
