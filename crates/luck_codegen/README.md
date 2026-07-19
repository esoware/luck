# luck_codegen

Code generation from Lua ASTs back to source text. Supports Lua 5.1–5.5 and Luau.

## Overview

`luck_codegen` is the luck toolchain's reverse parser. It walks an AST and emits minimal valid source code. It is source-independent: every leaf's text — identifiers, numbers, strings, and Luau types — comes from token-carried values on the AST, not from slicing the original source. The bundler, minifier, and any tool that mutates the AST go through this crate to produce final output.

## Key Features

- **Compact output** — minimal valid code. Strips comments, uses minimal whitespace, and inserts smart separators only where ambiguity requires them. Produces idempotent output: `compact(compact(x)) == compact(x)`.
- **Source-independent** — leaf text is read from the tokens stored on AST nodes, so the printer needs no access to the original bytes. Luau type annotations are emitted by walking the real `Type` AST, not by re-slicing source spans.
- **40+ ambiguity cases handled** — the separator system covers every place Lua's grammar allows two adjacent tokens to merge into the wrong meaning.
- **Statement boundary disambiguation** — inserts semicolons when a previous statement ends with `)` or `}` and the next begins with `(`, preventing the parser from re-reading the second statement as a function call on the first.

## Architecture

### Compact Printer

`compact.rs` is a tree-walking printer. Each AST variant has an emitter that calls the printer's spacing primitives and recurses into children. Output is built into a single growing string with minimal copying.

### Separator Logic

`separator.rs` is a token adjacency analyzer. It answers: given the last token emitted and the next one queued, is a space required? The cases it handles include:

- `a..b` vs `a ..b` (concat vs number-prefix ambiguity in 5.x)
- `a-b` vs `a -b` (subtraction vs unary minus near comment starts)
- `a<<b` (Lua 5.3+ shift operator collisions)
- Keyword boundaries (`returnx` would lex as one identifier)

Each case is enumerated rather than derived from a general rule, because Lua's lexer rules differ enough by version that a unified rule would be wrong somewhere.
