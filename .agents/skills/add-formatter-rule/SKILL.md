---
name: add-formatter-rule
description: Changes how a Lua/Luau construct is formatted by luck_formatter - layout, packing, breaking, hug patterns, comment placement - while preserving the idempotency invariant. Use when asked to format X differently, wrap long calls, fix indentation, or for any edit under crates/luck_formatter/.
---

# Change a formatter rule

The formatter is a Wadler/Prettier-style engine: emitters are `impl Format
for Node` blocks composing combinators into a flat tag-stream IR
(`src/ir.rs` - read it for the real element set, it is authoritative); the
printer (`src/printer.rs`) decides which groups fit flat and which expand.
Emitters live in `src/format_*.rs`, one module per construct family; comment
placement lives only in `comments.rs` and `format_block.rs`.

Leaf text comes from token-carried values (`src/tokens.rs`), never from
source slices - that is what lets `format_block` format synthetic ASTs. Never
push raw whitespace strings; the printer owns layout.

## Gotchas that produce real bugs

- **Comment safety.** If the construct can contain comments (tables, arg
  lists, chains), verify a comment inside it survives in place - comment
  relocation is this crate's historical worst bug class. Comments not visited
  by an emitter are drained after the statement by `emit_trailing_comments`.
  Never consume comments inside `best_fitting` variants.
- **Synthetic ASTs.** `format_block` runs with no source text and
  `Comments::none()`/`Comments::synthetic(..)`. Any new logic that reads
  source must degrade gracefully when it is `None` (see the blank-line policy
  in `format_block.rs`).
- **Hug patterns**: a single function or table literal argument stays inline
  (`format_function.rs`), defeated by a magic trailing comma.
- **Fill mode**: simple all-positional tables pack greedily.
- **Access chains** break at method calls, not field dots.
- **Types are real AST** (`luck_ast::types::Type`) - extend `format_type.rs`
  impls, never re-tokenize text.

## Tests

In `crates/luck_formatter/src/tests/` (or `tests/it/`), cover:

1. Re-parses - `parse(format(src))` error-free, including at width 1 (the
   narrowest layout must still be valid syntax).
2. Idempotent - `format(format(src)) == format(src)`, the non-negotiable
   invariant.
3. AST-equivalent - run the construct through `format_and_verify`.
4. Multiple widths - 60, 80, 120.
5. Format-off survives - `-- luck: format off` ... `-- luck: format on`
   regions come through byte-for-byte (note the exact directive syntax).
6. Synthetic path - if the construct has a `synth` constructor: build ->
   `format_block` -> parse -> `blocks_equiv`.

## Gate and bump

`just lint && just test -p luck_formatter -p luck_testgen` (testgen runs the
idempotency property tests). Layout tweak = patch, new directive or option =
minor: follow `.agents/skills/bump-versions/SKILL.md`.
