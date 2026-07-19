# luck_formatter

Prettier-style code formatter for Lua 5.1–5.5 and Luau.

## Overview

Each AST node's emitter is an `impl Format`, composing combinators (`group`, `indent`, `soft_line`, `fill`, `best_fitting`, `if_group_breaks`, …) into a tag-stream IR. A line-width-aware printer then decides which groups fit flat and which need to expand.

## Key Features

- **Formats ASTs directly** — the primary entry point, `format_block`, formats an AST with no source text required, so programmatically constructed ASTs (e.g. decompiler output) and synthetic comments format the same as parsed source.
- **Comment preservation** — comments are tracked separately and reinserted at their original positions. `-- luck: format off` / `-- luck: format on` directives suppress formatting for a region.
- **Hug patterns** — a single function or table argument inside a call stays inline rather than forcing the outer group to expand.
- **Access chain breaking** — long method chains (`foo:bar():baz()`) break at each call with proper indentation.
- **Smart condition breaking** — multi-line `if` and `while` conditions drop unnecessary outer parentheses when they expand.
- **Fill mode** — simple table constructors pack entries greedily onto lines instead of one-per-line.
- **Range formatting** — `format_range` formats only statements overlapping a byte range, emitting the rest verbatim. Useful for editor "format selection".
- **Verified formatting** — `format_and_verify` re-parses its own output and compares the AST to the original, catching any structure-altering bug before it ships.
- **Luau type annotations** — full formatting of Luau type syntax by walking the real `Type` AST, with group-based line breaking.

## Architecture

### Pipeline

1. **Parse** — source text becomes an AST through `luck_parser`. (Skipped when a caller hands `format_block` an AST directly.)
2. **IR generation** — each node's `impl Format` runs against a `Formatter`, which records a tag stream of `FormatElement`s and `Tag`s (`group`, `indent`, `align`, soft/hard lines, `best_fitting` variants, group-id-addressable conditionals, and labels). Combinators compose these rather than hand-building a `Vec<FormatElement>`.
3. **Print** — a `propagate_expand` pre-pass marks every group containing a forced break as expanded, then the printer walks the stream with a mode stack, measuring each unforced group's content against the remaining width and committing to flat or expanded (fits-then-commit).

### IR Primitives

`group` decisions are local — a nested group can stay flat while its parent expands. `soft_line` becomes a space when flat and a newline when expanded; `hard_line` always becomes a newline. `indent` increases indentation for nested content, and `align` adds a fixed-width alignment. `fill` packs entries greedily. `best_fitting` supplies ordered variants and prints the first that fits. `if_group_breaks` emits content conditionally on a specific group's break decision (addressed by group id), and labels tag regions for post-passes.

### Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `line_width` | `u16` | 100 | Target line width before breaking |
| `indent_style` | `IndentStyle` | `Tabs` | Tabs or Spaces |
| `indent_width` | `u8` | 4 | Spaces per indent level or tab display width |
| `quote_style` | `QuoteStyle` | `Double` | Double or Single quotes |
| `hexadecimal_case` | `HexCase` | `Preserve` | Preserve, Lower, or Upper — case of hex digits `A`–`F`; the `0x` prefix and exponent markers are always lowercased regardless |
| `call_parentheses` | `CallParentheses` | `Always` | Always, NoSingleString, NoSingleTable, None (single string or table), or Input (preserve source) |
| `collapse_simple_statement` | `CollapseSimpleStatement` | `Never` | Never, FunctionOnly, ConditionalOnly, or Always |
| `line_endings` | `LineEndings` | `Unix` | Unix (LF) or Windows (CRLF) |
| `block_newline_gaps` | `BlockNewlineGaps` | `Never` | Never (strip) or Preserve blank lines at block start/end |
| `space_after_function_names` | `SpaceAfterFunction` | `Never` | Never, Definitions, Calls, or Always — space between callee/`function` and `(` |
| `sort_requires` | `bool` | `false` | Sort `require` statements (source-level pre-pass) |
| `magic_trailing_comma` | `bool` | `false` | A trailing comma forces the surrounding table/call list to break |

### Module Layout

| Module | Role |
|--------|------|
| `lib.rs` | Public API (`format_block`, `format`, `format_range`, `format_and_verify`) and option types |
| `ir.rs` | `Format` trait, `Formatter`, and the tag-stream IR (`FormatElement`, `Tag`, combinators) |
| `printer.rs` | Line-width-aware IR printer (expand pre-pass + fits-then-commit) |
| `comments.rs` | Comment interleaving (sourced + synthetic) and `format off`/`on` region handling |
| `format_expr.rs` | Expression formatting |
| `format_stmt.rs` | Statement formatting |
| `format_table.rs` | Table constructor formatting |
| `format_function.rs` | Function definition and call formatting |
| `format_block.rs` | Block and body formatting |
| `format_type.rs` | Luau type annotation formatting |
| `quotes.rs` | Quote style normalization |
| `numbers.rs` | Numeric-literal normalization |
| `tokens.rs` | Token-to-IR text emission |
| `sort_requires.rs` | `require` statement sorting (source-level pre-pass) |
| `ast_equiv.rs` | AST-equality verifier backing `format_and_verify` |
