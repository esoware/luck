# luck_semantic

Scope analysis, symbol resolution, and version- and environment-aware standard library definitions for Lua and Luau.

## Overview

`luck_semantic` builds a `ScopeTree` from a parsed AST, tracking variable declarations, references, shadowing, and upvalue captures across function boundaries. It also ships the version- and environment-aware stdlib catalog (Lua 5.x, standalone Luau, and Roblox Luau) the linter and LSP use to reason about built-in functions.

## Key Features

- **Scope tree** — full lexical scoping model with Module, Function, Block, and Loop scope kinds.
- **Reference classification** — every identifier use is recorded as Read, Write, or ReadWrite.
- **Shadowing detection** — declarations that reuse a name from an outer scope are flagged.
- **Upvalue tracking** — captures across function boundaries are recorded with source and destination scopes.
- **Version-aware stdlib** — every standard library function carries an argument-count signature, a purity flag, and version-gated availability across Lua 5.1–5.5 and Luau.

## Architecture

### Scope Tree

The tree models lexical scoping with four scope kinds:

- **Module** — top-level scope of a file.
- **Function** — function body. Names captured from outer scopes become upvalues.
- **Block** — `do…end`, `then`, and `else` blocks.
- **Loop** — `for`, `while`, and `repeat` bodies.

Each scope owns its locally declared symbols and tracks every reference to them.

### Symbols and References

A `Symbol` is a declared variable with a `SymbolKind` (Local, Parameter, ForVariable, …) and the span of its declaration. A `Reference` is a usage site with a `ReferenceKind` (Read, Write, ReadWrite — the last covers `x += 1` in Luau and similar compound assignments).

Names not declared in any enclosing scope are unresolved — they reach the global scope, where they may match a stdlib entry or remain unknown.

### Standard Library

`stdlib_model.rs` enumerates the built-in functions of each Lua version. Each entry includes:

- **`FunctionSignature`** — minimum and maximum argument counts.
- **Purity** — whether the function is free of observable side effects, used by the minifier to know whether a call can be dropped or hoisted.
- **Version gating** — entries are filtered by the active `LuaVersion`, so `string.pack` shows up for Lua 5.3+ but not 5.1.

### Module Layout

| File | Purpose |
|------|---------|
| `lib.rs` | Public API and `analyze()` entry point |
| `scope.rs` | Scope tree data structures |
| `builder.rs` | AST visitor that constructs the scope tree |
| `stdlib_model.rs` | Version-gated standard library definitions |
