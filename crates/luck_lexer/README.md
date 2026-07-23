# luck_lexer

Single-pass tokenizer for Lua 5.1-5.5 and Luau.

## Overview

The lexer reads source bytes once and produces a flat `Vec<Token>` plus a parallel `Vec<Comment>`. It never backtracks and never panics: malformed input becomes a `SourceError`, the cursor advances, and lexing continues.

Two entry points share the same machine:

- `lex(source, version) -> LexResult` materializes the whole token stream, the comment array, and any errors. This is what one-shot consumers (LSP, benches) call.
- `Lexer::new(source, version)` then `next_token()` pulls tokens on demand; the parser drives it this way so it never allocates a full token vector. Comments and errors accumulate on the lexer and are drained with `finish()` once the caller reaches EOF.

## Key features

- **Version-gated tokens** - keyword and operator recognition queries `LuaVersion` predicates. `//` is floor division only from Lua 5.3+; `goto` is a keyword only from 5.2+; `global` from 5.5; `&`, `|`, `~`, `<<`, `>>` arrive with bitwise ops in 5.3+; compound assignment, `@`, `?`, backtick strings, and binary/underscore-separated numbers are Luau-only. In Luau, `~` also starts a negation type and doubled angle tokens form explicit type instantiation.
- **Complete number literals** - decimal, hexadecimal, hex floats (5.2+), binary literals and underscore separators (Luau), plus Luau's exact signed-decimal/full-bit-pattern 64-bit integer literals with an `i` suffix.
- **Complete string literals** - short strings with single or double quotes, long brackets (`[==[...]==]`) at any equals-sign depth, and the full escape repertoire (`\x`, `\z`, `\u{}`, decimal escapes), each escape gated to the versions that accept it.
- **Luau interpolated strings** - backtick-delimited strings with `{expr}` segments, split into `InterpBegin` / `InterpMid` / `InterpEnd` tokens with brace depth tracked across nested expressions so the parser sees a consistent begin/end shape.
- **Shebang and BOM** - a leading UTF-8 BOM is skipped, and a `#`-prefixed first line is captured as a shebang comment rather than parsed as the length operator.

## Architecture

### Cursor (`cursor.rs`)

A zero-copy forward-only byte cursor: `peek`, `peek_at`, `advance`, `rest`, and batched `advance_until_match`, all tracking a single byte position.

### Batched scanning (`search.rs`)

String and comment bodies are scanned through a 256-entry stop-byte table (`ByteMatchTable`, built at compile time by `byte_match_table!`). `find_match` walks a scalar prefix, then fixed-size batches that vectorize, so long runs stay branch-light. This is the crate's hot path and exists for measured reasons.

### Token production

`lexer.rs` is the state machine: lead-byte dispatch, keyword recognition, comment classification, shebang handling, and interpolated-string brace tracking. `number.rs` and `string.rs` handle the two literal families with nontrivial sub-grammars; the long-bracket close scan is shared between long strings and block comments.

### Comment stream

Comments are buffered as they are encountered and emitted into the comment array with surrounding-whitespace metadata. A comment on the same line as a preceding token (no newline between) is `Trailing`; anything at the start of a line, or before the first token, is `Leading`. The lexer sets each comment's `attached_to` byte offset directly - the start of the following token for leading comments, the start of the preceding token for trailing ones.

### Error recovery

On a malformed token the lexer pushes a `SourceError`, advances past the offending byte (consuming a full UTF-8 sequence so a multi-byte character yields one error), and resumes. There is no fatal tier; the parser sees whatever tokens were produced and reports downstream errors against them.
