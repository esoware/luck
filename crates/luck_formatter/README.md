# luck_formatter

Prettier-style code formatter for Lua 5.1-5.5 and Luau.

## Overview

Each AST node's emitter is an `impl Format`, composing combinators (`group`, `indent`, `soft_line`, `fill`, `if_group_breaks`, ...) into a tag-stream IR. A line-width-aware printer then decides which groups fit flat and which need to expand.

## Key features

- **Formats ASTs directly.** `format_block` formats an AST with no source text required, so ASTs built in code, and their synthetic comments, format the same as parsed source. It returns a `FormatError` for a tree nested past `luck_ast::MAX_NESTING_DEPTH`, which only a tree built in code can be, because the parser would refuse the output. It also returns one for a tree holding a leaf with no text that reads back as itself where it sits, such as a keyword token where a name belongs or an error node, at that leaf. Every leaf's text comes through `luck_ast`'s `*_spelling` and `check_*` functions, which `luck_codegen` uses too, so both printers refuse the same trees with the same error.
- **Comment preservation.** Comments before, after, and between statements lay out normally. Table constructors and call argument lists claim the comments written between their items: the list breaks one item per line, and each comment keeps the line it was on. A single-line block comment between a statement's condition and its `then` or `do` prints where it was written. Each construct claims only the comments inside its own span, so none is dragged across an opening delimiter. A comment after a block's last statement, or alone in an empty block, stays in that block, and blank lines around comments are kept. When an interior comment still has no safe place, the smallest statement around it is kept verbatim instead of moving the comment. Its lines are re-indented to its block and a run of blank lines collapses to one, but nothing else changes, and the inside of a long string or long comment stays exactly as written. `-- luck: format off` and `-- luck: format on` copy the region between them byte for byte, directive lines included, and a region that doesn't line up with a block's statements keeps the whole statement around it verbatim. `-- luck: ignore` (or `-- luck: format ignore`) keeps the next statement's layout and line breaks, re-indented to its block. A directive's last word must end at a word boundary, so `-- luck: format offset` is not one.
- **Hug patterns.** A call's function and table arguments are hugged: when the call is too long for one line, the other arguments stay beside the parentheses and those arguments break instead, as in `table.sort(items, function(a, b)` or `createElement("Frame", {` ... `}, {` ... `})`. A hugged table that fits stays on one line beside a function, which always breaks. An empty table isn't hugged. A call where another argument sits between two hugged ones, or one whose lines would still be too long, puts every argument on its own line. StyLua byte-for-byte compatibility isn't a goal.
- **Access chain breaking.** A long method chain (`foo:bar():baz()`) breaks before each `:` call, with the continuation lines indented. A dot chain of three or more names breaks before each `.` only when the names don't fit after what they follow, so `end).never.to.throw()` stays on the line a callback closes on.
- **Condition breaking.** `if`, `elseif`, `while`, and `until` conditions lose redundant outer parentheses, and a long `if`, `elseif`, or `while` condition breaks onto its own indented line before `then` or `do`. An operator chain still too long for that line breaks before each operator, level with its first operand.
- **Operator chains.** A long chain breaks before each operator of one precedence level, and an operand that binds tighter breaks only when it doesn't fit by itself, so `a == b or c == d` breaks at `or` alone.
- **Assignments.** Values too long for the `=` line move to the next line, one indent deeper, when they fit there, and otherwise hang from `=`. A lone table, function, or call stays on the `=` line and breaks inside its own brackets. This covers `local`, `global`, plain, and compound assignments.
- **If-expressions.** A Luau if-expression too long for one line starts an indented line at each `then`, `elseif`, and `else`.
- **Fill mode.** A table of positional literals and variables packs entries greedily onto lines instead of one per line.
- **Blank lines in tables.** A blank line between two fields is kept, as one between statements is, and breaks the table. A packed list stays packed on either side of it. The lines right inside the braces take none.
- **Range formatting.** `format_range` formats only the statements overlapping a byte range, or those of the innermost nested block holding the whole range, and copies every other byte as it was, line endings included. This is what an editor's "format selection" calls.
- **Verified formatting.** `format_and_verify` checks re-parsing, AST equivalence (string literals compare by the bytes they decode to in the target dialect), comment text (normalizing line endings and trailing whitespace), comment position among the tokens both texts keep, and second-pass idempotency. It aligns the two token streams, skipping a paren only one side has, up to the first other mismatch, such as a leading `|` the formatter adds to or drops from a Luau union. Past that point, comments are checked by text and order only. It isn't a target VM compiler or a proof of runtime equivalence.
- **Luau type annotations.** Luau type syntax formats from the `Type` AST, with group-based line breaking. A union or intersection that opens an array field with no access modifier keeps its leading `|` or `&` even when flat if its first member starts with `read` or `write`, which would otherwise read back as the field's access modifier.

## Architecture

### Pipeline

1. **Parse.** Source text becomes an AST through `luck_parser`. (Skipped when a caller hands `format_block` an AST directly.)
2. **IR generation.** Each node's `impl Format` writes into a `Formatter`, which records a flat stream of `FormatElement`s (text, soft and hard lines) with paired `Tag`s marking groups, indents, fills, line suffixes, and conditionals keyed by group id. Emitters build it with combinators rather than pushing elements by hand.
3. **Print.** A `propagate_expand` pre-pass marks every group containing a forced break as expanded, then the printer walks the stream with a mode stack, measuring each unforced group's content against the remaining width and committing to flat or expanded (fits-then-commit).

### IR primitives

`group` decisions are local: a nested group can stay flat while its parent expands. `soft_line` prints nothing when flat and a newline when expanded, `soft_line_or_space` prints a space when flat, and `hard_line` is always a newline. `indent` increases indentation for nested content. `fill` packs entries greedily, and `separated` writes a list with a separator between its items. `if_group_breaks` emits content only when a given group, addressed by its id, breaks.

### Configuration

The defaults follow the [Roblox Lua style guide](https://roblox.github.io/lua-style-guide/).

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `line_width` | `u16` | 100 | Target line width before breaking |
| `indent_style` | `IndentStyle` | `Tabs` | Tabs or Spaces |
| `indent_width` | `u8` | 4 | Columns per indent level: the spaces written per level, or how wide a tab counts against `line_width` |
| `quote_style` | `QuoteStyle` | `Double` | Double, Single, AutoPreferDouble, or AutoPreferSingle (the auto styles switch quotes when that needs fewer escapes) |
| `hexadecimal_case` | `HexCase` | `Preserve` | Case of hex digits `A`-`F`: Preserve, Lower, or Upper. The `0x`/`0b` prefix and exponent markers are always lowercased |
| `call_parentheses` | `CallParentheses` | `Always` | Always, NoSingleString, NoSingleTable, None (single string or table), or Input (preserve source) |
| `collapse_simple_statement` | `CollapseSimpleStatement` | `Never` | Never or ConditionalOnly. Function bodies never collapse, though an empty one keeps `end` on the header's line, as in `function() end`. An `if` that doesn't fit on one line, or whose body spans lines or has a comment that couldn't keep its place on one line, stays expanded |
| `line_endings` | `LineEndings` | `Unix` | Unix (LF) or Windows (CRLF) |
| `block_newline_gaps` | `BlockNewlineGaps` | `Never` | Never (strip) or Preserve blank lines at block start/end |
| `space_after_function_names` | `SpaceAfterFunction` | `Never` | Never, Definitions, Calls, or Always, for a space before a definition's parameter list, a call's `(`, or both |
| `sort_requires` | `bool` | `true` | Sort runs of `local x = require(...)` and `GetService(...)` statements by name, where the require takes a string or a dotted path, leaving alone any run whose reordering can't be proven safe |
| `magic_trailing_comma` | `bool` | `true` | A trailing comma keeps the table broken: one field per line, or still packed when the source packed its positional values several to a line (argument lists can't carry one) |

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
| `tokens.rs` | Name tokens' IR text |
| `sort_requires.rs` | `require` statement sorting, over the lines of formatted text |
| `ast_equivalence.rs` | AST-equality verifier backing `format_and_verify`, with its comparisons in `ast_equivalence/` split by node family as `luck_ast` is: `stmt.rs`, `expr.rs`, `function.rs`, and `types.rs` |
| `verify.rs` | `format_and_verify`: the re-parse, AST, comment-placement, and idempotency checks |
