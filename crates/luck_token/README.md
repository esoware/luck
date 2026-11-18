# luck_token

The types and rules every other luck crate builds on, except `luck_allocator`: positions, tokens, dialects, and how Lua reads literals and line breaks.

## Usage

- `Span`: a byte range with `u32` offsets, which caps a source at 4 GiB.
- `Token` and `TokenKind`: a token holds only its kind, its span, and two flags. `Token::text` slices its text from the source.
- `LuaVersion`: the syntax of Lua 5.1 to 5.5 or Luau. Each difference between versions is a `has_<feature>` predicate, such as `has_goto`.
- `StdlibEnvironment`: standalone or Roblox, which only Luau's standard library tells apart.
- `Comment`: a comment the lexer found, with its kind, whether it leads or trails a token, and the token it attaches to.
- `BinOp`, `UnOp`, and `CompoundOp`: the operators, and each binary operator's precedence and associativity.
- `SourceError`: the one error type of lexing, parsing, and formatting, a span and a message.
- `parse_lua_number`, `decode_string`, and `encode_string`: what a numeral or a string literal means, shared by every stage that reads or writes one.
- `line_breaks`, `has_line_break`, and `LineIndex`: what counts as a line break in each version, and line and column lookup.
- `CodeBuffer`: the output buffer the printers write into.

```rust
use luck_token::{LuaVersion, Span, Token, TokenKind};

assert!(LuaVersion::Lua54.has_goto());
assert_eq!(Span::new(0, 2).merge(Span::new(5, 7)), Span::new(0, 7));

let source = "`hello {name}`";
let begin = Token {
    span: Span::new(0, 8),
    kind: TokenKind::InterpolatedStringBegin,
    has_line_break_before: true,
    is_unterminated: false,
};
assert_eq!(begin.text(source), "hello ");
```
