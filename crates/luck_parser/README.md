# luck_parser

Pratt expression parser with recursive-descent statement parsing for Lua 5.1–5.5 and Luau.

## Overview

The parser consumes a token stream from `luck_lexer` and produces an AST from `luck_ast`. It is depth-limited, error-recovering, and version-gated. It never panics on malformed input — every error becomes a `SourceError` and parsing continues to the next synchronization point.

## Key Features

- **Pratt expression parsing** — binding power tables encode operator precedence correctly across every Lua version, including the multi-step precedence climb required for `or` / `and` / comparisons / bitwise / arithmetic / unary / power.
- **Recursive descent for statements** — control flow, declarations, and assignments parse top-down with explicit context markers.
- **Depth limiting** — caps recursion to prevent stack overflow on pathological input.
- **Error recovery** — on failure, the parser pushes a `SourceError` and synchronizes at the next statement keyword (`if`, `while`, `for`, `local`, etc.), allowing multiple errors to surface in a single pass.
- **Context stack** — tracks the active parsing context ("if-statement", "for-loop", "function declaration") so diagnostics report where a problem occurred, not just what.
- **Version-gated syntax** — goto/labels (5.2+), bitwise operators (5.3+), local attributes (5.4+), generalized iteration (5.5+), and the Luau extensions, including the full Luau type grammar.

## Architecture

### Pipeline

The parser walks the token stream produced by `luck_lexer`. `parser.rs` holds the cursor, the context stack, the depth counter, and the block-parsing loop. `expr.rs` implements the Pratt parser. `stmt.rs` implements the recursive-descent dispatch over statement keywords.

### Expression Parsing

The Pratt parser uses left and right binding powers for each operator. Each call to `parse_expression(min_bp)` consumes a prefix, then iterates infix and postfix forms as long as their left binding power exceeds `min_bp`. This handles function calls, indexing, method calls, table constructors, and Luau type casts uniformly.

### Statement Parsing

Each compound statement (`if`, `for`, `while`, `repeat`, function declarations) pushes a context label before parsing its body and pops it afterward. When recovery synchronizes, the context stack tells the diagnostic where in the program the cursor was.

### Luau Type Annotations

`luau.rs` is a full recursive-descent parser for the Luau type grammar. It produces real `luck_ast::types::Type` nodes — not opaque spans — for every annotation site (`x: T`, `<T>`, function return types, type-declaration bodies). The grammar covers unions and intersections (including the leading-separator multiline form `| A | B`), optionals (`T?`), table types, function types (`(params) -> R`), generic lists with defaults, type packs, `typeof(expr)`, singletons, and variadic packs. Precedence runs loosest to tightest: union `|`, intersection `&`, postfix `?`, primary. The nested-generic cases where `>>` and `>=` must be split back into closing angle brackets are handled during parsing. Codegen reconstructs types by walking these typed nodes; there is no source slicing.
