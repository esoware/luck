# luck_codegen

Code generation from Lua ASTs back to source text. Supports Lua 5.1–5.5 and Luau.

## Overview

`luck_codegen` is the luck toolchain's reverse parser. It walks an AST and emits minimal valid source code. It is source-independent: every leaf's text — identifiers, numbers, strings, and Luau types — comes from token-carried values on the AST, not from slicing the original source. The bundler, minifier, and any tool that mutates the AST go through this crate to produce final output.

## Key Features

- **Compact output** — minimal valid code. Strips comments, uses minimal whitespace, and inserts smart separators only where ambiguity requires them. Produces idempotent output: `compact(compact(x)) == compact(x)`.
- **Source-independent** — leaf text is read from the tokens stored on AST nodes, so the printer needs no access to the original bytes. Luau type annotations are emitted by walking the real `Type` AST, not by re-slicing source spans.
- **Token-merge disambiguation** — the separator system inserts a space wherever two adjacent emitted pieces would otherwise merge into a different token, covering cases like `--` (comment start), `//` (floor division), `[[` (long-string open), `..`/`...` (concat/vararg), and `<<`/`>>`/`>=`.
- **Statement boundary disambiguation** — inserts a semicolon when a statement whose first token is `(` follows one that could be read as a call prefix, preventing the parser from re-reading the second statement as a continuation of the first.

## Architecture

### Compact Printer

`compact.rs` is a tree-walking printer. Each AST variant has an emitter that calls the printer's spacing primitives and recurses into children. Output is built into a `luck_token::code_buffer::CodeBuffer` (a byte buffer with an ASCII fast path), sized to the source length as a capacity hint since compact output is never longer than its input.

### Separator Logic

`separator.rs` tracks the previously emitted piece as a one-byte `PrevClass` (word, number, `-`, `/`, `[`, `.`, `..`, `<`, `>`, `;`, or other) instead of a cloned token, and `needs_space` answers whether the next piece's first byte would merge with it into something else: two words merging into one identifier, `--` forming a comment, `//` forming floor division, `[[` opening a long string, `..`/`...` colliding with concat or vararg, and `<<`/`>>`/`>=` forming shift or comparison tokens. Each case is enumerated rather than derived from a general rule, because Lua's lexer rules differ enough by version that a unified rule would be wrong somewhere.

## Testing

Public-API and round-trip tests live in `tests/it/` as a single binary (`compact.rs`, `roundtrip.rs`). `separator.rs` additionally keeps inline white-box tests at the bottom of the file.
