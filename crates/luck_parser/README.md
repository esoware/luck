# luck_parser

Pratt expression parser with recursive-descent statement parsing for Lua 5.1-5.5 and Luau.

## Overview

The parser consumes a token stream from `luck_lexer` and produces an AST from `luck_ast`. It is depth-limited, error-recovering, and version-gated. It never panics on malformed input. Every error becomes a `SourceError`, and parsing continues to the next synchronization point.

## Key features

- **Pratt expression parsing.** One precedence table covers every Lua version, from `or` and `and` through comparisons, bitwise operators, concatenation, arithmetic, unary operators, and power. Bitwise operators only bind where the version has them.
- **Recursive descent for statements.** Control flow, declarations, and assignments parse top-down.
- **Depth limiting.** Nesting caps at 256 levels to protect the host stack. This is a tool resource limit, not a dialect grammar rule or an emulation of a VM's configurable compiler limits, and deeply nested valid programs can exceed it.
- **Error recovery.** On failure, the parser records a `SourceError` and skips to the next statement start or block boundary, so one pass reports many errors.
- **Construct-aware errors.** A compound statement that is missing one of its own keywords says which construct expected it: ``missing `then` in if-statement``.
- **Version-gated syntax.** Goto and labels (5.2+), bitwise operators (5.3+), local attributes (5.4+), `global` declarations, leading attributes, and named varargs (5.5), plus the Luau extensions, including 64-bit integer literals, explicit type instantiation, value exports, and the full Luau type grammar.

## Architecture

### Pipeline

The parser pulls tokens from `luck_lexer` one at a time, with one token of lookahead. `parser.rs` holds the cursor, the depth counter, the function scope (whether `...`, `break`, and `continue` are valid where the parser is, and whether a `return` there is the module's), the chunk entry point, and the shared recovery primitives (`expect`, `expect_identifier`, and `expect_identifier_recover`, which record the error themselves and hand back the current span, nothing, or a placeholder token, so parsing continues). `stmt.rs` holds the block-parsing loop, the recursive-descent dispatch over statement keywords, control flow, and assignments. `declaration.rs` parses function statements and the `local`, `const`, `global`, `export`, and `type` declarations, with the attributed name lists they share and Luau's module-level export rules. `expr.rs` implements the Pratt parser, with the table constructors that statements share, and `function.rs` parses the function bodies and parameter lists that function statements and expressions share. `attributes.rs` owns the attribute micro-grammar for both dialects and its parse-time validation: Lua `<const>`/`<close>` variable attributes, Luau `@native`/`@[deprecated(...)]` function attributes. `types.rs` parses Luau types. `validate.rs` runs the opt-in post-parse scope checks, one module each: const writes (`const_writes.rs`), goto/label resolution (`gotos.rs`), and Luau continue/until (`continue_until.rs`).

### Expression parsing

Each binary operator has a precedence level and an associativity, defined in `luck_token`. `parse_expression(min_precedence)` parses a unary operator or a primary expression with its suffix chain (field access, indexing, calls, method calls, Luau type instantiation) and any Luau `::` cast, then loops over binary operators whose precedence is at least `min_precedence`. A left-associative operator parses its right operand one level higher, so chains like `a + b + c` grow in the loop, while the right-associative `..` and `^` recurse, under the depth limit.

### Statement parsing

When a compound statement (`if`, `while`, `do`, `for`, `repeat`) is missing one of its own keywords, such as `then`, `do`, or `end`, the error names the statement: ``missing `then` in if-statement``. Other missing tokens, such as a function body's `end` or a closing bracket, report `expected X, found Y`.

### Luau type annotations

`types.rs` is a full recursive-descent parser for the Luau type grammar. It produces real `luck_ast::Type` nodes, not opaque spans, for every annotation site (`x: T`, `<T>`, function return types, type-declaration bodies). The grammar covers unions and intersections (including the leading-separator multiline form `| A | B`), optionals (`T?`), table types with `read`/`write` access, function types (`(params) -> R`), generic lists with defaults, type packs, `typeof(expr)`, singletons, and variadic and generic packs. The lexer reads `>>` and `>=` greedily, so the parser splits off the leading `>` when it closes a generic list. Codegen reconstructs types by walking these typed nodes, with no source slicing.

## Testing

Public-API tests live in `tests/it/` as a single binary: one file per dialect (`lua51.rs`...`lua55.rs`, `luau.rs`) plus `luau_types.rs` for the type grammar, `version.rs` for syntax gating across versions, `errors.rs`, `depth.rs`, `validate.rs`, `parse_result.rs` for what `parse` hands back, and `fixtures.rs`, which sweeps the shared `tests/fixtures/` corpus at the repo root.
