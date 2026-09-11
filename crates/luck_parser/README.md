# luck_parser

Pratt expression parser with recursive-descent statement parsing for Lua 5.1-5.5 and Luau.

## Overview

The parser consumes a token stream from `luck_lexer` and produces an AST from `luck_ast`. It is depth-limited, error-recovering, and version-gated. It never panics on malformed input; every error becomes a `SourceError`, and parsing continues to the next synchronization point.

## Key features

- **Pratt expression parsing.** Binding power tables encode operator precedence across every Lua version, including the multi-step precedence climb for `or`, `and`, comparisons, bitwise, arithmetic, unary, and power.
- **Recursive descent for statements.** Control flow, declarations, and assignments parse top-down with explicit context markers.
- **Depth limiting.** Recursive parser work caps at 256 entries to protect the host stack. This is a tool resource limit, not a dialect grammar rule or an emulation of a VM's configurable compiler limits; deeply nested valid programs can exceed it.
- **Error recovery.** On failure, the parser pushes a `SourceError` and synchronizes at the next statement keyword (`if`, `while`, `for`, `local`, and the rest), so one pass reports many errors.
- **Context stack.** The parser tracks the active context ("if-statement", "for-loop", "function declaration") so diagnostics report where a problem occurred, not only what.
- **Version-gated syntax.** Goto and labels (5.2+), bitwise operators (5.3+), local attributes (5.4+), generalized iteration (5.5+), and the Luau extensions, including 64-bit integer literals, explicit type instantiation, value exports, and the full Luau type grammar.

## Architecture

### Pipeline

The parser walks the token stream produced by `luck_lexer`. `parser.rs` holds the cursor, the context stack, the depth counter, the block-parsing loop, and the shared recovery primitives (`expect`/`expect_identifier_recover`, which record one error and hand back a placeholder span or token so parsing continues). `expr.rs` implements the Pratt parser. `stmt.rs` implements the recursive-descent dispatch over statement keywords. `attributes.rs` owns the attribute micro-grammar for both dialects and its parse-time validation: Lua `<const>`/`<close>` variable attributes, Luau `@native`/`@[deprecated(...)]` function attributes. `validate.rs` holds the opt-in post-parse scope checks (const writes, goto/label resolution, Luau continue/until).

### Expression parsing

The Pratt parser uses left and right binding powers for each operator. Each call to `parse_expression(min_bp)` consumes a prefix, then iterates infix and postfix forms as long as their left binding power exceeds `min_bp`. This handles function calls, indexing, method calls, table constructors, and Luau type casts uniformly.

### Statement parsing

Each compound statement (`if`, `for`, `while`, `repeat`, function declarations) pushes a context label before parsing its body and pops it afterward. When recovery synchronizes, the context stack tells the diagnostic where in the program the cursor was.

### Luau type annotations

`luau.rs` is a full recursive-descent parser for the Luau type grammar. It produces real `luck_ast::types::Type` nodes, not opaque spans, for every annotation site (`x: T`, `<T>`, function return types, type-declaration bodies). The grammar covers unions and intersections (including the leading-separator multiline form `| A | B`), optionals (`T?`), table types, function types (`(params) -> R`), generic lists with defaults, type packs, `typeof(expr)`, singletons, and variadic packs. Parsing splits `>>` and `>=` back into closing angle brackets for the nested-generic cases that need it. Codegen reconstructs types by walking these typed nodes; there is no source slicing.

## Testing

Public-API tests live in `tests/it/` as a single binary: one file per Lua version (`lua51.rs`...`lua55.rs`, `luau.rs`, `luau_types.rs`), plus `errors.rs`, `depth.rs`, `validate.rs`, and `fixtures.rs`, which sweeps the shared `tests/fixtures/` corpus at the repo root.
