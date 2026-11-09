# luck_token

Foundation types for the luck toolchain: spans, tokens, version flags, and the unified error type.

## Overview

Every other luck crate depends on `luck_token`. It defines the building blocks that flow through the compiler pipeline: positions in source, language variant flags, the token shape produced by the lexer, the operators the AST stores, the comment shape attached separately from the AST, and the single error type all stages emit. Every item is exported from the crate root.

The crate has zero internal dependencies. Its only external dependency is `compact_str` for inline string storage.

## Key features

- **Compact spans.** `Span` uses `u32` start and end, half the size of a `usize` pair. Source files cap at 4 GB.
- **Version-gated feature flags.** `LuaVersion` exposes predicates like `has_goto()`, `has_bitwise_ops()`, `has_attributes()`, `has_floor_div()`. Downstream crates branch on capabilities, not version numbers.
- **Inline string storage.** `TokenKind` payloads use `CompactString`, which stores up to 24 bytes inline with no heap allocation. Most Lua identifiers fit.
- **Single error type.** `SourceError` carries a span and a message. `LexError`, `ParseError`, and `FormatError` are type aliases for it, so errors flow end-to-end without conversion.
- **Shared literal semantics.** `parse_lua_number`, `decode_string`, and `encode_string` convert literal token text to runtime values and back, so every crate that folds or re-emits literals uses one implementation instead of re-parsing raw text. `decode_string` borrows the token text when it already is the value, so an escape-free string decodes without allocating, and `encode_string` returns the `CompactString` a token carries, so a short literal encodes without allocating either. `NumberSubtypes::from` picks a version's number model: Lua 5.3+ splits integers from floats, everything else is f64. `short_string_body` strips a quoted literal's quotes without decoding its escapes, for code that wants the spelling rather than the value.
- **One definition of a line break.** `line_breaks`, `has_line_break`, `ends_with_line_break`, and `strip_line_break` are the only place that decides what ends a line of Lua source. Every crate that counts, splits, or classifies source lines goes through them, because the dialects disagree: PUC Lua reads LF, CR, CRLF, and LFCR as one line break each, and Luau counts only LF. `chunk_start` and `is_lua_whitespace` do the same job for a leading BOM and for whitespace.
- **Comments as data.** `Comment` records its kind (line, single- or multi-line block, shebang), its position (leading, trailing), and `attached_to`, the start offset of the token the lexer attached it to. `leading_hot_comments` picks out the `--!` comments in the run that opens a chunk.
- **Byte-level output buffer.** `CodeBuffer` wraps a `Vec<u8>` behind a UTF-8 invariant, giving `luck_codegen` and `luck_formatter` ASCII-byte pushes without per-character UTF-8 encoding, while `into_string` stays zero-copy.

## Architecture

### Span and SourceError

A `Span` is a `(u32, u32)` byte range in the original source. Every diagnostic and AST node carries one. AST nodes embed several spans each, so the narrow width adds up on large programs.

`SourceError` carries a span and a message. That covers lexer, parser, and formatter failures, so downstream crates alias it instead of defining their own.

### LuaVersion

`LuaVersion` is an enum with one variant per supported dialect (Lua 5.1 through 5.5 and Luau). Consumers ask its predicate methods instead of matching variants, so a new Lua release adds a variant and turns on its features in the existing predicates.

### TokenKind

`TokenKind` has one variant per token, and most carry no payload. Identifiers, numbers, strings, and interpolated-string segments carry their source text as a `CompactString`, which skips a heap allocation for short text. Every other kind but `Eof` has a fixed spelling, which `static_text` returns for error messages and the formatter alike. `TokenKind::keyword` goes the other way for keywords, under a version: it is the lexer's keyword table, and the AST synthesizer asks it whether a name is reserved. `TokenKind::is_reserved` adds Luau's contextual `continue` to that table, for the linter and the minifier's renamer, which keep both out of the names they write. `TokenKind::is_identifier_shaped` checks the spelling alone, an ASCII letter or `_` and then letters, digits, and `_`, which keywords share, so a name check pairs it with a keyword check such as one of those two.

### Operators

`BinOp`, `UnOp`, and `CompoundOp` are the one-byte operator enums AST nodes hold in place of a token. Each maps from its token kind with `from_token_kind` and back to its spelling with `static_text`, and `BinOp::precedence`, with `UnOp::PRECEDENCE` for the unary level, is the table the parser, the minifier, and the AST synthesizer share. `BinOp::can_nest_bare` reads the parenthesization rule off it: a binary operand prints bare when it binds tighter than its parent, or as tight on the `OperandSide` the parent associates toward.

### Comments

Comments are not part of the AST. The lexer emits them into a separate `Vec<Comment>` sorted by source position. Each comment records whether a newline precedes it since the last token (`is_preceded_by_newline`), which decides its position, and whether it ends its line (`is_followed_by_newline`). The lexer also sets `attached_to` to the start of a neighboring token: the next one for a leading comment, the previous one for a trailing comment.
