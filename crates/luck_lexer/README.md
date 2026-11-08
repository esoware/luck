# luck_lexer

Single-pass tokenizer for Lua 5.1-5.5 and Luau.

## Overview

The lexer reads source bytes once and produces a flat `Vec<Token>` plus a separate `Vec<Comment>`. It never backtracks and never panics: malformed input becomes a `SourceError`, the cursor advances, and lexing continues.

Two entry points share the same machine:

- `lex(source, version) -> LexResult` materializes the whole token stream, the comment array, and any errors. One-shot consumers such as the LSP, the formatter, and the benches call this.
- `Lexer::new(source, version)` then `next_token()` pulls tokens on demand. The parser drives it this way, so it never allocates a full token vector. Comments and errors accumulate on the lexer and are drained with `finish()` once the caller reaches EOF.

## Key features

- **Version-gated tokens.** Keyword and operator recognition queries `LuaVersion` predicates. `//` is floor division in Lua 5.3+ and Luau. `goto` is a keyword in 5.2+ (not Luau) and `global` in 5.5. `&`, `|`, `~`, `<<`, `>>` arrive with bitwise ops in 5.3+, and Luau also lexes `&` and `|` for its type syntax. Compound assignment, `->`, `@`, `?`, backtick strings, and binary/underscore-separated numbers are Luau-only. In Luau, standalone `~` is invalid (`~=` remains valid), and `<<` and `>>` stay two angle tokens each, which the parser reads as explicit type instantiation.
- **Complete number literals.** Decimal, hexadecimal, hex floats (5.2+), binary literals and underscore separators (Luau), plus Luau's 64-bit integer literals with an `i` suffix, which must fit an `i64` in decimal but may use all 64 bits in hex and binary.
- **Complete string literals.** Short strings with single or double quotes, long brackets (`[==[...]==]`) at any equals-sign depth, and the full escape repertoire (`\x`, `\z`, `\u{}`, decimal escapes), each escape gated to the versions that accept it.
- **Luau interpolated strings.** Backtick-delimited strings with `{expr}` segments, split into `InterpolatedStringBegin` / `InterpolatedStringMiddle` / `InterpolatedStringEnd` tokens. A brace-depth stack finds the `}` that closes each expression, and a string with no expressions still lexes as an empty `InterpolatedStringBegin` plus an `InterpolatedStringEnd`, so the parser always sees a begin/end pair.
- **Shebang and BOM.** The lexer skips a leading UTF-8 BOM and captures a `#`-prefixed first line as a shebang comment rather than parsing it as the length operator.

## Architecture

### Cursor (`cursor.rs`)

A zero-copy forward-only byte cursor: `peek`, `peek_at`, `advance`, `rest`, and batched `advance_until_match`, all tracking a single byte position.

### Body scanning (`search.rs`, `string.rs`)

Interpolated-string text is scanned through a 256-entry stop-byte table (`ByteMatchTable`, built at compile time by `byte_match_table!`). `find_match` walks a scalar prefix, then fixed-size batches of scalar table lookups, so long runs stay branch-light without relying on vectorization. Short strings check the same kind of table over their first 8 bytes, where most strings end, then switch to `memchr` for the closing quote, escape, or line feed, with a second `memchr` for a carriage return bounded to that run. Comments and long brackets scan with `memchr` directly. This is the crate's hot path, and its design comes from measurement.

### Token production

`lexer.rs` is the state machine: lead-byte dispatch, comment classification, shebang handling, and interpolated-string brace tracking. Its `lex_*` methods each consume one token or comment. Keywords come from `luck_token`'s `TokenKind::keyword`, which the AST synthesizer and the linter share. `number.rs` and `string.rs` hold the literal sub-grammars as `scan_*` functions over the cursor, each returning the token kind or a `LexError`: numbers, short and long strings with their escapes, and the raw segments of an interpolated string. The long-bracket close scan is shared between long strings and block comments.

### Comment stream

The lexer buffers comments as it encounters them and emits them into the comment array with surrounding-whitespace metadata. A comment on the same line as a preceding token (no newline between) is `Trailing`. Anything at the start of a line, or before the first token, is `Leading`. The lexer sets each comment's `attached_to` byte offset directly: the start of the following token for leading comments, the start of the preceding token for trailing ones.

### Error recovery

On a malformed token the lexer pushes a `SourceError`, advances past the offending byte (consuming a full UTF-8 sequence so a multi-byte character yields one error), and resumes. The parser sees whatever tokens were produced and reports downstream errors against them. The only fatal case is input over 4 GiB, which `u32` spans can't address: `Lexer::new` refuses it, and `lex` returns that one error and no tokens.

## Testing

Public-API tests live in `tests/it/` as a single binary, split by concern (`tokens.rs`, `numbers.rs`, `strings.rs`, `interpolation.rs`, `comments.rs`), with each file covering its tokens' version gating too. `streaming.rs` checks that `next_token` streams exactly what `lex` materializes, over the shared `tests/fixtures/` corpus. The batched `find_match` scan has white-box tests in `search.rs`.
