# luck_lexer

Single-pass tokenizer for Lua 5.1–5.5 and Luau.

## Overview

The lexer reads source bytes once and produces a flat `Vec<Token>` plus a parallel `Vec<Comment>`. It never backtracks and never panics. Malformed input becomes a `SourceError`; the cursor advances and lexing continues.

## Key Features

- **Version-gated tokens** — keyword and operator recognition queries `LuaVersion` predicates. `//` is floor division only from Lua 5.3+; `goto` is a keyword only from 5.2+; `&`, `|`, `~`, `<<`, `>>` arrive with bitwise ops in 5.3+.
- **Complete number literals** — decimal, hexadecimal, hex floats (5.2+), binary literals (Luau), and underscore separators (Luau).
- **Complete string literals** — short strings with single or double quotes, long brackets (`[==[…]==]`) at any equals-sign depth, and the full escape repertoire (`\x`, `\z`, `\u{}`, decimal escapes).
- **Luau interpolated strings** — backtick-delimited strings with `{expr}` segments. The lexer splits these into `InterpBegin` / `InterpMid` / `InterpEnd` tokens and tracks brace depth across nested expressions so the parser sees a consistent shape.
- **Shebang support** — a `#!` prefix on byte 0 is captured as a shebang comment, not parsed as the length operator.

## Architecture

### Cursor

`cursor.rs` is a thin zero-copy byte cursor with `peek`, `peek_at`, `advance`, and position tracking. The lexer commits to a token after exactly one byte of lookahead — anything beyond that is a sign the design needs a longer match in a dedicated submodule.

### Token Production

`lexer.rs` is the main state machine: keyword recognition, comment classification, shebang handling, and interpolated-string brace tracking. `number.rs` and `string.rs` handle the two literal families that have nontrivial sub-grammars. Each module produces a `Token` and updates the cursor in lockstep.

### Comment Stream

Comments are buffered as the lexer encounters them and emitted into the comment array with surrounding-whitespace metadata. Position classification (`Leading`, `Trailing`, `Standalone`) follows simple rules: a comment on the same line as a preceding token, with no newline between, is trailing; anything else is leading.

The `attached_to` link is filled by a later AST-walking pass — the lexer does not know which syntactic node a comment belongs to.

### Error Recovery

On a malformed token, the lexer emits a `SourceError`, skips the offending byte, and resumes. There is no fatal tier; the parser sees whatever the lexer produced and reports downstream errors against it.
