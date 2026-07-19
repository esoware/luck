---
name: add-formatter-rule
description: Changes how a Lua/Luau construct is formatted by luck_formatter - expression/statement layout, table packing, function calls, parameter lists, type annotations, comment placement, line-width breaking, hug patterns, fill mode, access chain breaking - while preserving the idempotency invariant. Use when asked to format X differently, wrap long calls, fix indentation, break long chains, hug arguments, or for any edit under crates/luck_formatter/.
argument-hint: <construct-name>
allowed-tools: Read, Edit, Write, Grep, Glob, Bash(cargo:*)
---

# Change a formatter rule

The formatter is a Wadler/Prettier-style engine: emitters are `impl Format for
Node` blocks composing **combinators** into a flat tag-stream IR; a printer
walks the stream deciding which groups fit flat and which expand. Read
`crates/luck_formatter/src/ir.rs` for the real element set and combinator
API before writing anything - it is authoritative.

Leaf text comes from **token-carried values** (`crate::tokens`), never from
source slices. This is what lets `format_block` format synthetic
ASTs; do not add source-text dependence to emitters.

## Layout

| Where | What |
|---|---|
| `crates/luck_formatter/src/lib.rs` | `format_block()` (AST-in, primary), `format()`, `format_range()`, `format_and_verify()` |
| `crates/luck_formatter/src/ir.rs` | `FormatElement`/`Tag`, the `Format` trait, `Formatter`, all combinators, `write!` |
| `crates/luck_formatter/src/printer.rs` | line-width-aware printer (`propagate_expand` pre-pass + fits-then-commit) |
| `crates/luck_formatter/src/tokens.rs` | token -> IR text (`write_token`, `FormatToken`) |
| `crates/luck_formatter/src/format_expr.rs` | expressions, binary/access chains |
| `crates/luck_formatter/src/format_stmt.rs` | statements, conditions (`Condition` strips redundant parens) |
| `crates/luck_formatter/src/format_table.rs` | table constructors (fill mode, magic trailing comma) |
| `crates/luck_formatter/src/format_function.rs` | `FormatFunctionBody`, calls, hug pattern, param lists |
| `crates/luck_formatter/src/format_block.rs` | statement loop, comment/verbatim protocol, blank-line policy |
| `crates/luck_formatter/src/format_type.rs` | Luau types from the real `Type` AST |
| `crates/luck_formatter/src/comments.rs` | dual-anchored comments (sourced byte-offset cursor / synthetic node-anchored), format-off ranges |
| `crates/luck_formatter/src/quotes.rs` | quote normalization |
| `crates/luck_formatter/src/sort_requires.rs` | source-level require sorting (pre-parse rewrite) |
| `crates/luck_formatter/src/ast_equiv.rs` | AST-equality verifier behind `format_and_verify` |

## The emission idiom

Emitters implement `Format` and compose combinators; sequences use tuples
or `crate::write!`:

```rust
impl Format for WhileLoop {
    fn fmt(&self, f: &mut Formatter) {
        crate::write!(
            f,
            [group((
                token("while"),
                indent((soft_line_or_space(), Condition(&self.condition))),
                soft_line_or_space(),
                token("do"),
            ))]
        );
        // ...
    }
}
```

Key combinators (all in `ir.rs`): `token("static")`, `text(dynamic)`,
`space()`, `soft_line()`, `soft_line_or_space()`, `hard_line()`,
`empty_line()`, `group(..)`, `group_with_id(id, ..)` (ids from
`f.group_id()`), `indent(..)`, `if_group_breaks(id, ..)` /
`if_group_fits(id, ..)` / `indent_if_group_breaks(id, ..)` (conditionals
reference **any** already-started group by id - trailing commas, break-
coupled layouts), `fill(mode, entries)` (greedy packing),
`best_fitting(variants)` (most-flat first; first that fits wins),
`expand_parent()`, `line_suffix(..)`, `format_with(|f| ..)` for closures.
Speculation: `f.checkpoint()` / `f.restore(cp)` / `f.will_break_since(cp)`
- comment state is checkpoint-safe, but never consume comments inside
`best_fitting` variants.

Options are on `f.options`; tokens print via
`crate::tokens::{write_token, FormatToken}`. Never push raw whitespace
strings; the printer owns layout.

## Steps

### 1. Find the right module

See the table above. Comment placement lives in `comments.rs` and
`format_block.rs` (the statement loop is their only consumer), never in
the per-construct files.

### 2. Honor the special features

- **Hug patterns.** A single function or table literal argument stays
  inline (`format_function.rs`), defeated by a magic trailing comma.
- **Fill mode.** Simple all-positional tables pack greedily onto lines.
- **Access chain breaking.** Chains break at method calls, not field dots.
- **Comment safety.** If your construct can contain comments (tables,
  arg lists, chains), verify a comment inside it survives in place -
  comment relocation is this crate's historical worst bug class. Comments
  not visited by an emitter are drained after the statement by
  `emit_trailing_comments`.
- **Synthetic ASTs.** `format_block` runs with `Comments::none()` /
  `Comments::synthetic(..)` and **no source text** - any new logic that
  reads `f.comments.source_text()` must degrade gracefully when it is
  `None` (see the blank-line policy in `format_block.rs`).
- **Types are real AST** (`luck_ast::types::Type`) - extend
  `format_type.rs` impls, never re-tokenize text.

### 3. Tests

In `crates/luck_formatter/src/tests/`, add cases that verify:

1. **Re-parses.** `parse(format(src))` is error-free - including at
   width 1 (the narrowest layout must still be valid syntax; trailing
   commas after `...` in Luau params once violated this).
2. **Idempotent.** `format(format(src)) == format(src)` - the
   non-negotiable invariant.
3. **AST-equivalent.** Run the construct through `format_and_verify` -
   layout must never change program meaning.
4. **Multiple line widths.** Test at 60, 80, 120.
5. **Format-off survives.** `-- luck: format off` ... `-- luck: format on`
   regions come through byte-for-byte (note the directive syntax -
   `luck-format-off` does not exist).
6. **Synthetic path.** If the construct has a `synth` constructor, add a
   `tests/synthetic.rs` case: build -> `format_block` -> parse ->
   `blocks_equiv`.

### 4. Gate

```sh
cargo clippy -p luck_formatter --all-targets -- -D warnings
cargo test -p luck_formatter
cargo test -p luck_testgen   # property tests: idempotency + AST-in path
```

### 5. Version bump

Patch for layout tweaks. Minor for new directives or config options.
Use `/bump-versions`.

The VS Code extension in `editors/vscode/` consumes the formatter through
`luck_cli`; no extension edit is needed unless the CLI interface changes.
