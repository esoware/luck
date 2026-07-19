# luck_token

Foundation types for the luck toolchain — spans, tokens, version flags, and the unified error type.

## Overview

Every other luck crate depends on `luck_token`. It defines the building blocks that flow through the compiler pipeline: positions in source, language variant flags, the token shape produced by the lexer, the comment shape attached separately from the AST, and the single error type all stages emit.

The crate has zero internal dependencies. Its only external dependency is `compact_str` for inline string storage.

## Key Features

- **Compact spans** — `Span` uses `u32` start and end. Source files are capped at 4 GB; every AST node shrinks by half compared to `usize` spans.
- **Version-gated feature flags** — `LuaVersion` exposes predicates like `has_goto()`, `has_bitwise_ops()`, `has_attributes()`, `has_integer_division()`. Downstream crates branch on capabilities, not version numbers.
- **Inline string storage** — `TokenKind` payloads use `CompactString`. Strings ≤24 bytes live on the stack; most Lua identifiers fit.
- **Single error type** — `SourceError` is the only error struct in the toolchain. `LexError`, `ParseError`, and `FormatError` are type aliases for it, so errors flow end-to-end without conversion.
- **First-class comments** — `Comment` records kind (line, block, shebang), position (leading, trailing, standalone), and an `attached_to` slot the parser fills during AST construction.

## Architecture

### Span and SourceError

A `Span` is a `(u32, u32)` byte range in the original source. Every diagnostic and AST node carries one. The narrow width matters at scale: AST nodes embed multiple spans, and across a million-node program the savings compound.

`SourceError` carries a span, a message, and optional help text. It is expressive enough to cover lexer, parser, and formatter failures, which is why downstream crates type-alias rather than redefine.

### LuaVersion

`LuaVersion` is an enum with one variant per supported language flavor (Lua 5.1 through 5.5 and Luau). Behavior is queried through predicate methods rather than by matching variants directly, which decouples consumers from the version axis — a new Lua release adds a variant and lights up its features through the existing predicates.

### TokenKind

Token kinds are enumerated exhaustively. Each variant carries the minimum payload it needs, often nothing. String payloads use `CompactString` to skip a heap allocation for the common case of short identifiers.

### Comments

Comments are not part of the AST. The lexer emits them into a parallel `Vec<Comment>` sorted by source position. Each comment records the surrounding whitespace context the formatter needs (`preceded_by_newline`, `followed_by_newline`). The parser sets `attached_to` while building the AST, linking each comment to the syntactic node it documents.
