---
name: add-formatter-rule
description: Changes how luck_formatter formats a Lua/Luau construct, covering layout, packing, breaking, hug patterns, and comment placement, while preserving the idempotency invariant. Use when asked to format X differently, wrap long calls, fix indentation, or for any edit under crates/luck_formatter/.
---

# Change a formatter rule

The formatter is a Wadler/Prettier-style engine: emitters are `impl Format for Node` blocks composing combinators into a flat tag-stream IR (`src/ir.rs`, the authority on the real element set). The printer (`src/printer.rs`) decides which groups fit flat and which expand. Emitters live in `src/format_*.rs`, one module per construct family. Comment placement lives only in `comments.rs` and `format_block.rs`.

Leaf text comes from token-carried values (`src/tokens.rs`), never from source slices. That is what lets `format_block` format synthetic ASTs. Never push raw whitespace strings, because the printer owns layout.

## Gotchas that produce real bugs

- **Comment safety.** If the construct can contain comments (tables, arg lists, chains), verify a comment inside it survives in place. Comment relocation is this crate's historical worst bug class. A comment no emitter claimed sends the statement down `write_verbatim_or`'s fallback.
- **Synthetic ASTs.** `format_block` runs with no source text and `Comments::none()`/`Comments::synthetic(..)`. Any new logic that reads source must degrade gracefully when it is `None` (see the blank-line policy in `format_block.rs`).
- **Hug patterns.** A call's function and table arguments break in the argument list's place (`format_function.rs`, through the hugged groups in `printer.rs`), unless another argument sits between two of them.
- **Fill mode.** Simple all-positional tables pack greedily.
- **Access chains** break at method calls, not field dots.
- **Types are real AST** (`luck_ast::Type`), so extend the `format_type.rs` impls and never re-tokenize text.

## Tests

White-box cases go in the owning file's `#[cfg(test)] mod tests` (render a node with `crate::fixtures::render`). Public-API cases go in `tests/it/` through `assert_format`/`assert_format_with` (`assert_format_range` for a selection), which pin the exact output and check that it re-parses and formats to itself. Nothing checks at runtime that the output means what the input does, so the expected text is the proof and needs the review code gets. Cover:

1. Multiple widths: 1, 60, 80, 120. The narrowest layout must still be valid syntax.
2. Comments inside the construct, at each of those widths.
3. Format-off survives: `-- luck: format off` ... `-- luck: format on` regions come through byte-for-byte (note the exact directive syntax).
4. Synthetic path: if the construct has a `Synth` constructor, build it and pin its output through `synthetic.rs`'s `assert_format_block`.
5. Fixture snapshots: `tests/it/fixtures.rs` snapshots every shared fixture at several widths. Run `cargo insta test -p luck_formatter`, review each changed snapshot as a diff, and accept it with `just accept`.

## Gate and bump

`just lint && just test -p luck_formatter -p luck_testgen` (testgen runs the idempotency property tests). Layout tweak = patch, new directive or option = minor: follow `.agents/skills/bump-versions/SKILL.md`.
