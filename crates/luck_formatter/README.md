# luck_formatter

Prettier-style code formatter for Lua 5.1-5.5 and Luau.

## Overview

Each AST node's emitter is an `impl Format`, composing combinators (`group`, `indent`, `soft_line`, `fill`, `best_fitting`, `if_group_breaks`, ...) into a tag-stream IR. A line-width-aware printer then decides which groups fit flat and which need to expand.

## Key features

- **Formats ASTs directly.** The main entry point, `format_block`, formats an AST with no source text required, so programmatically constructed ASTs and synthetic comments format the same as parsed source.
- **Comment preservation.** Supported comment positions lay out normally. Table constructors and call argument lists claim the comments written between their items: the list breaks one item per line, and each comment keeps the line it was on. A statement header does the same for a single-line block comment left between its condition and its `then`/`do`, printing it where it was written. Each construct claims only what its own span covers, so no comment is dragged across an opening delimiter. If an interior comment still has no safe layout, the smallest enclosing statement is preserved verbatim rather than relocated, re-anchored to its block's indentation and otherwise as written, apart from the usual collapse of a blank-line run to one. A newline inside a long string or long comment is content, so the lines around it are never re-anchored. This deliberately prioritizes fidelity over reformatting that statement. `-- luck: format off` / `-- luck: format on` directives suppress formatting for a region.
- **Hug patterns.** A single function or table argument inside a call stays inline rather than forcing the outer group to expand. Multi-argument calls use a uniform expanded list when needed, including calls ending in a callback or table. StyLua byte-for-byte compatibility is not a goal.
- **Access chain breaking.** Long method chains (`foo:bar():baz()`) break at each call, with the continuation lines indented.
- **Condition breaking.** Multi-line `if` and `while` conditions drop redundant outer parentheses when they expand.
- **Fill mode.** Simple table constructors pack entries greedily onto lines instead of one per line.
- **Range formatting.** `format_range` formats only the statements overlapping a byte range and emits the rest verbatim. This is what an editor's "format selection" calls.
- **Verified formatting.** `format_and_verify` checks re-parsing, AST equivalence, comment text (normalizing line endings and trailing whitespace), comment position among the tokens both texts keep, and second-pass idempotency. It aligns the two token streams so a paren only one side has is skipped, while one both sides keep is a real boundary. It is not a target VM compiler or a proof of runtime equivalence.
- **Luau type annotations.** Full formatting of Luau type syntax by walking the real `Type` AST, with group-based line breaking.

## Architecture

### Pipeline

1. **Parse.** Source text becomes an AST through `luck_parser`. (Skipped when a caller hands `format_block` an AST directly.)
2. **IR generation.** Each node's `impl Format` runs against a `Formatter`, which records a tag stream of `FormatElement`s and `Tag`s (`group`, `indent`, `align`, soft/hard lines, `best_fitting` variants, and group-id-addressable conditionals). Combinators compose these rather than hand-building a `Vec<FormatElement>`.
3. **Print.** A `propagate_expand` pre-pass marks every group containing a forced break as expanded, then the printer walks the stream with a mode stack, measuring each unforced group's content against the remaining width and committing to flat or expanded (fits-then-commit).

### IR primitives

`group` decisions are local: a nested group can stay flat while its parent expands. `soft_line` becomes a space when flat and a newline when expanded; `hard_line` always becomes a newline. `indent` increases indentation for nested content, and `align` adds a fixed-width alignment. `fill` packs entries greedily. `best_fitting` supplies ordered variants and prints the first that fits. `if_group_breaks` emits content conditionally on a specific group's break decision (addressed by group id).

### Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `line_width` | `u16` | 100 | Target line width before breaking |
| `indent_style` | `IndentStyle` | `Tabs` | Tabs or Spaces |
| `indent_width` | `u8` | 4 | Spaces per indent level or tab display width |
| `quote_style` | `QuoteStyle` | `Double` | Double or Single quotes |
| `hexadecimal_case` | `HexCase` | `Preserve` | Preserve, Lower, or Upper; case of hex digits `A`-`F`. The `0x` prefix and exponent markers are always lowercased regardless |
| `call_parentheses` | `CallParentheses` | `Always` | Always, NoSingleString, NoSingleTable, None (single string or table), or Input (preserve source) |
| `collapse_simple_statement` | `CollapseSimpleStatement` | `Never` | Never, FunctionOnly, ConditionalOnly, or Always |
| `line_endings` | `LineEndings` | `Unix` | Unix (LF) or Windows (CRLF) |
| `block_newline_gaps` | `BlockNewlineGaps` | `Never` | Never (strip) or Preserve blank lines at block start/end |
| `space_after_function_names` | `SpaceAfterFunction` | `Never` | Never, Definitions, Calls, or Always; space between callee/`function` and `(` |
| `sort_requires` | `bool` | `false` | Sort `require` statements (source-level pre-pass) |
| `magic_trailing_comma` | `bool` | `false` | A trailing comma forces the surrounding table to break (argument lists cannot carry one) |

### Module layout

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
