# luck_token

The types and rules every luck crate that reads or writes Luau builds on: positions, tokens, how Luau reads literals, line breaks, and what opens a chunk, and its compiler's limits.

A source is bytes, as Luau reads it, so its strings and comments need not be UTF-8. Its names and numerals are ASCII.

## Usage

- `Span`: a byte range with `u32` offsets, which caps a source at 4 GiB.
- `Token` and `TokenKind`: a token holds only its kind, its span, and three flags. `Token::text` slices its bytes from the source. A contextual keyword, such as `type` or `continue`, has a kind of its own, and `TokenKind::is_name` counts it as a name. `TokenKind::keyword` reads a name's kind, and `TokenKind::of_name` reads it from a name's first eight bytes and its length with no branch, for a lexer that reads many names at once.
- `Comment`: a comment the lexer found, with its kind, whether it leads or trails a token, the token it attaches to, and its `CommentContent`, what it tells a tool: prose, a hot comment, or a `luck:` directive.
- `CommentContent::of` and `LintDirective::parse`: the one grammar of `luck:` directives, which the lexer reads each comment's content with, and the linter each lint directive's rules. A malformed directive parses to a `DirectiveError`, which the linter reports.
- `BinOp`, `UnOp`, and `CompoundOp`: the operators, each binary operator's precedence and associativity, and the binary operator each compound one applies.
- `read_numeral`, `read_escape`, and `decode_string`: what a numeral, an escape, or a string literal means, which the lexer checks and the parser decodes with. A number's value is an `f64`, and a string's is bytes.
- `spell_number`, `spell_string`, `write_long_string`, and `write_interpolated_text`: the shortest literal that reads back as a value, for the printers. A number spells on the stack, and a string measures itself before it writes. `spell_string_shorter_than` spells one only when that beats a length, such as the literal's own, and stops measuring as soon as it can't.
- `line_breaks`, `has_line_break`, and `LineIndex`: what counts as a line break, which is only an LF, so a lone CR is whitespace, and line and column lookup.
- `has_shebang`, `leading_hot_comments`, and `chunk_header`: what a chunk opens with before its code, which is a first line its loader skips and the hot comments that take effect only there, and the header a printer that drops comments writes to keep them.
- `MAX_ACTIVE_LOCALS` and `MAX_REGISTERS`: Luau's compiler limits on the locals one function holds at once and the registers it uses, which the linter and the minifier keep code under.

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
