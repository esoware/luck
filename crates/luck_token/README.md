# luck_token

Tokens, spans, operators, and literals of the Luau language.

This crate defines the pieces that a Luau lexer, parser, and printer share. A
`Token` holds a kind, a span, and a few flags, but no text, so a token stream
stays small and each token's text is sliced from the source with `Token::text`.

Sources are handled as bytes, the way Luau reads them, so strings and comments
don't have to be valid UTF-8. Spans use `u32` offsets, which limits a source to
4 GiB. Number and string literals are decoded and spelled exactly as Luau does.

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

* `Span`, a byte range in a source, with `index_to_offset` and
  `offset_to_index` to convert positions to and from its `u32` offsets.
* `LineIndex`, which maps offsets to lines and columns.
* `Token` and `TokenKind`, every Luau token.
* `BinOp`, `UnOp`, and `CompoundOp`, the operators, and `Associativity`.
* `read_numeral` and `spell_number`, which read and write number literals, with
  `Numeral` and `NumberSpelling`.
* `decode_string`, `read_escape`, `spell_string`, and
  `spell_string_shorter_than`, which read and write string literals, with
  `StringValue`, `StringKind`, `StringSpelling`, `Escaped`, and `EscapeError`.
* `decode_interpolated_text`, `write_interpolated_text`, and
  `write_long_string`, for interpolated and long strings.
* `Comment`, with its `CommentKind` and `CommentPosition`, and
  `CommentContent`, what a comment tells a tool.
* `LintDirective`, a `luck:` lint directive, with `LintVerb`, `LintExtent`,
  `DirectiveError`, and `DirectiveErrorKind`.
* `line_breaks`, `has_line_break`, `ends_with_line_break`, `strip_line_break`,
  and `is_luau_whitespace`, which follow Luau's rules for line breaks and
  whitespace.
* `has_shebang`, `leading_hot_comments`, and `chunk_header`, which find the
  shebang and `--!` comments a chunk opens with.
* `MAX_ACTIVE_LOCALS` and `MAX_REGISTERS`, Luau's compiler limits.
