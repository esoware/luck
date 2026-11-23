# luck_token

The types and rules every other luck crate builds on, except `luck_allocator` and `luck_data_structures`: positions, tokens, dialects, and how Lua reads literals and line breaks.

A source is bytes, as PUC Lua and Luau read it, so its strings and comments need not be UTF-8. Its names and numerals are ASCII.

## Usage

- `Span`: a byte range with `u32` offsets, which caps a source at 4 GiB.
- `Token` and `TokenKind`: a token holds only its kind, its span, and two flags. `Token::text` slices its bytes from the source. A contextual keyword, such as Luau's `type` or Lua 5.5's `global`, has a kind of its own where its version has it, and `TokenKind::is_name` counts it as a name.
- `LuaVersion`: the syntax of Lua 5.1 to 5.5 or Luau. Each difference between versions is a `has_<feature>` predicate, such as `has_goto`.
- `LuaTarget`: the dialect a user names, Lua 5.1 to 5.5, Luau, or Luau on Roblox, which are the only valid pairs of a syntax and a host. It parses from names such as `5.4`, `luau`, or `roblox`, `version()` gives its syntax, and `is_roblox()` says whether its standard library is Roblox's. Whatever needs the standard library takes a `LuaTarget`, and whatever reads only the syntax takes its `LuaVersion`. The `serde` feature derives `Deserialize` for the lowercase names `luck.json` takes, `lua51` to `lua55`, `luau`, and `roblox`, and `schemars` derives `JsonSchema` too.
- `Comment`: a comment the lexer found, with its kind, whether it leads or trails a token, the token it attaches to, and its `CommentContent`, what it tells a tool: prose, a Luau hot comment, or a `luck:` directive.
- `CommentContent::of` and `LintDirective::parse`: the one grammar of `luck:` directives, which the lexer reads each comment's content with, and the linter each lint directive's rules. A malformed directive parses to a `DirectiveError`, which the linter reports.
- `BinOp`, `UnOp`, and `CompoundOp`: the operators, and each binary operator's precedence and associativity.
- `read_numeral`, `read_integer`, `read_escape`, and `decode_string`: what a numeral, an escape, or a string literal means in each version, which the lexer checks and the parser decodes with. `LuaNumber` is a number's value, an integer or a float, and a string's is bytes. `LuaNumber::to_integer` gives the integer a float equals, as Lua 5.3+ converts one for a bitwise operator or a table key, and `LuaNumber::literal_magnitude` the magnitude a literal of a number holds, under a `-` when it's negative.
- `spell_number`, `spell_integer`, `spell_string`, `write_long_string`, and `write_interpolated_text`: the shortest literal that reads back as a value, for the printers. A number spells on the stack, and a string measures itself before it writes. `spell_string_shorter_than` spells one only when that beats a length, such as the literal's own, and stops measuring as soon as it can't.
- `line_breaks`, `has_line_break`, and `LineIndex`: what counts as a line break in each version, and line and column lookup.

```rust
use luck_token::{LuaVersion, Span, Token, TokenKind};

assert!(LuaVersion::Lua54.has_goto());
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
