# luck_semantic

Scope analysis, symbol resolution, and per-environment standard library definitions for Lua and Luau.

## Overview

`luck_semantic` builds a `ScopeTree` from a parsed AST, tracking variable declarations, references, shadowing, and upvalue captures across function boundaries. It also ships the stdlib catalog the linter and LSP use to reason about built-ins: one complete, independent library per environment (Lua 5.1-5.5, standalone Luau, Roblox Luau).

## Key Features

- **Scope tree** — full lexical scoping model with Module, Function, Block, and Loop scope kinds.
- **Reference classification** — every identifier use is recorded as Read, Write, or ReadWrite.
- **Shadowing detection** — declarations that reuse a name from an outer scope are flagged.
- **Upvalue tracking** — captures across function boundaries are recorded with source and destination scopes.
- **Per-environment stdlib** — typed signatures with overloads, deprecation metadata down to individual parameters and constant values, purity and must-use markers, and value shapes for method resolution.
- **Conservative shape resolution** — `resolve.rs` maps `local f = io.open(...)` / `game:GetService("Players")` chains to stdlib entries without a type-inference engine.

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

Each environment is one fully self-contained TOML file under `stdlib_data/`, selected by `(LuaVersion, StdlibEnvironment)`:

- `lua51.toml` … `lua55.toml` — one per numbered Lua version, verified against that version's reference manual.
- `luau.toml` — standalone (open-source) Luau.
- `luau_roblox.toml` — the Roblox runtime, plus two **generated** files spliced in at load: `roblox_api.toml` (service and class-name constant sets from the Roblox API dump) and `roblox_enums.toml` (the full `Enum` tree). Regenerate both with `cargo test -p luck_semantic regenerate_roblox_api -- --ignored`; never hand-edit them.

Both Luau catalogs include the distinct `integer` primitive's library surface, the matching `buffer.readinteger` / `buffer.writeinteger` APIs, and the current Luau math predicates/constants (`isnan`, `isinf`, `isfinite`, `nan`, `e`, `phi`, `sqrt2`, `tau`).

The files are deliberately independent — no inheritance or tier layering. Shared entries are duplicated and kept honest by the drift-guard suite in `tests/drift.rs`, which cross-checks every surface shared between files against an explicit allowlist of manual-verified divergences.

Entries model:

- **Overloaded signatures** — typed parameters with per-signature arity (`CFrame.new`, `collectgarbage`).
- **Shapes** — named member surfaces for non-global values (`file`, the derived `string` receiver, `Instance`, `DataModel`, `EnumItem`, …) with `extends` composition; entries declare the shape they return, so method chains resolve.
- **Constant parameters** — closed string-value sets (`game:GetService` service names, `Instance.new` class names, `collectgarbage` options), shareable across parameters via named `constant_sets`.
- **Deprecation** — at entry, parameter, and constant-value level, with `%n` replace templates for auto-fixes.
- **Purity / must-use / read-only** markers.

### Module Layout

| File | Purpose |
|------|---------|
| `lib.rs` | Public API and `analyze()` entry point |
| `scope.rs` | Scope tree data structures |
| `builder.rs` | AST visitor that constructs the scope tree |
| `stdlib_model.rs` | Stdlib data model, queries, and the per-environment library instances |
| `stdlib_load.rs` | TOML deserialization and composition into the `StdlibLibrary` model |
| `resolve.rs` | Conservative shape and callee resolution |
| `stdlib_data/` | Per-environment library data (+ generated Roblox data) |
