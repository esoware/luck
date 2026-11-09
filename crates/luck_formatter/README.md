# luck_formatter

Prettier-style code formatter for Lua 5.1-5.5 and Luau.

## Overview

Each AST node's emitter is an `impl Format`, composing combinators (`group`, `indent`, `soft_line`, `fill`, `if_group_breaks`, ...) into a tag-stream IR. A line-width-aware printer then decides which groups fit flat and which need to expand.

## Key features

- **Formats ASTs directly.** `format_block` formats an AST with no source text required, so ASTs built in code, and their synthetic comments, format the same as parsed source.
- **Comment preservation.** Comments before, after, and between statements lay out normally. Table constructors and call argument lists claim the comments written between their items: the list breaks one item per line, and each comment keeps the line it was on. A single-line block comment between a statement's condition and its `then` or `do` prints where it was written. Each construct claims only the comments inside its own span, so none is dragged across an opening delimiter. When an interior comment still has no safe place, the smallest statement around it is kept verbatim instead of moving the comment. Its lines are re-indented to its block and a run of blank lines collapses to one, but nothing else changes, and the inside of a long string or long comment stays exactly as written. `-- luck: format off` and `-- luck: format on` turn formatting off for a region, and `-- luck: ignore` (or `-- luck: format ignore`) keeps the next statement as written. A directive's last word must end at a word boundary, so `-- luck: format offset` is not one.
- **Hug patterns.** A single function or table argument inside a call stays inline rather than forcing the outer group to expand. Multi-argument calls use a uniform expanded list when needed, including calls ending in a callback or table. StyLua byte-for-byte compatibility isn't a goal.
- **Access chain breaking.** A long method chain (`foo:bar():baz()`) breaks before each `:` call, with the continuation lines indented.
- **Condition breaking.** `if`, `elseif`, `while`, and `until` conditions lose redundant outer parentheses, and a long `if`, `elseif`, or `while` condition breaks onto its own indented line before `then` or `do`.
- **Fill mode.** A table of positional literals and variables packs entries greedily onto lines instead of one per line.
- **Range formatting.** `format_range` formats only the statements overlapping a byte range and emits the rest verbatim. This is what an editor's "format selection" calls.
- **Verified formatting.** `format_and_verify` checks re-parsing, AST equivalence (string literals compare by the bytes they decode to in the target dialect), comment text (normalizing line endings and trailing whitespace), comment position among the tokens both texts keep, and second-pass idempotency. It aligns the two token streams, skipping a paren only one side has, up to the first other mismatch, such as a leading `|` the formatter adds to or drops from a Luau union. Past that point, comments are checked by text and order only. It isn't a target VM compiler or a proof of runtime equivalence.
- **Luau type annotations.** Luau type syntax formats from the `Type` AST, with group-based line breaking.

## Architecture

### Pipeline

1. **Parse.** Source text becomes an AST through `luck_parser`. (Skipped when a caller hands `format_block` an AST directly.)
2. **IR generation.** Each node's `impl Format` writes into a `Formatter`, which records a flat stream of `FormatElement`s (text, soft and hard lines) with paired `Tag`s marking groups, indents, fills, line suffixes, and conditionals keyed by group id. Emitters build it with combinators rather than pushing elements by hand.
3. **Print.** A `propagate_expand` pre-pass marks every group containing a forced break as expanded, then the printer walks the stream with a mode stack, measuring each unforced group's content against the remaining width and committing to flat or expanded (fits-then-commit).

### IR primitives

`group` decisions are local: a nested group can stay flat while its parent expands. `soft_line` prints nothing when flat and a newline when expanded, `soft_line_or_space` prints a space when flat, and `hard_line` is always a newline. `indent` increases indentation for nested content. `fill` packs entries greedily, and `separated` writes a list with a separator between its items. `if_group_breaks` emits content only when a given group, addressed by its id, breaks.

### Configuration

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `line_width` | `u16` | 100 | Target line width before breaking |
| `indent_style` | `IndentStyle` | `Tabs` | Tabs or Spaces |
| `indent_width` | `u8` | 4 | Spaces per indent level, unused with tabs, which count as one column when measuring |
| `quote_style` | `QuoteStyle` | `Double` | Double, Single, AutoPreferDouble, or AutoPreferSingle (the auto styles switch quotes when that needs fewer escapes) |
| `hexadecimal_case` | `HexCase` | `Preserve` | Case of hex digits `A`-`F`: Preserve, Lower, or Upper. The `0x`/`0b` prefix and exponent markers are always lowercased |
| `call_parentheses` | `CallParentheses` | `Always` | Always, NoSingleString, NoSingleTable, None (single string or table), or Input (preserve source) |
| `collapse_simple_statement` | `CollapseSimpleStatement` | `Never` | Never, FunctionOnly, ConditionalOnly, or Always. Function bodies never collapse, so FunctionOnly acts like Never and Always like ConditionalOnly. An `if` whose body has a comment that couldn't keep its place on one line stays expanded |
| `line_endings` | `LineEndings` | `Unix` | Unix (LF) or Windows (CRLF) |
| `block_newline_gaps` | `BlockNewlineGaps` | `Never` | Never (strip) or Preserve blank lines at block start/end |
| `space_after_function_names` | `SpaceAfterFunction` | `Never` | Never, Definitions, Calls, or Always, for a space before a definition's parameter list, a call's `(`, or both |
| `sort_requires` | `bool` | `false` | Sort runs of `local x = require(...)` and `GetService(...)` statements by name (source-level pre-pass) |
| `magic_trailing_comma` | `bool` | `false` | A trailing comma forces the table to break one field per line (argument lists can't carry one) |

### Module layout

| Module | Role |
|--------|------|
| `lib.rs` | Module list and public re-exports |
| `format.rs` | Public entry points (`format_block`, `format`, `format_range`) and `FormatResult` |
| `format_options.rs` | `FormatOptions` and its conversion from `FormatConfig` |
| `ir.rs` | `Format` trait, `Formatter`, and the tag-stream IR (`FormatElement`, `Tag`, combinators) |
| `printer.rs` | Line-width-aware IR printer (expand pre-pass + fits-then-commit) |
| `comments.rs` | Comment interleaving (sourced + synthetic), including the lists that claim the comments between their items |
| `directive.rs` | The `luck:` directives (`format off`/`on`, `ignore`) and the `format off` regions they mark |
| `format_expr.rs` | Expression formatting |
| `format_stmt.rs` | Statement formatting |
| `format_table.rs` | Table constructor formatting |
| `format_function.rs` | Function definition and call formatting |
| `format_block.rs` | Block formatting: the statement loop, blank lines, range selection, and verbatim statements |
| `format_type.rs` | Luau type annotation formatting |
| `quotes.rs` | Quote style normalization |
| `numbers.rs` | Numeric-literal normalization |
| `tokens.rs` | Token-to-IR text emission |
| `sort_requires.rs` | `require` statement sorting (source-level pre-pass) |
| `ast_equivalence.rs` | AST-equality verifier backing `format_and_verify`, with its comparisons in `ast_equivalence/` split by node family as `luck_ast` is: `stmt.rs`, `expr.rs`, `function.rs`, and `types.rs` |
| `verify.rs` | `format_and_verify`: the re-parse, AST, comment-placement, and idempotency checks |
