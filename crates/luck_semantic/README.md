# luck_semantic

Scope analysis, symbol resolution, and per-environment standard library definitions for Lua and Luau.

## Overview

`luck_semantic` builds a `ScopeTree` from a parsed AST, tracking variable declarations, references, shadowing, and upvalue captures across function boundaries. It also ships the stdlib catalog the linter and LSP use to reason about built-ins: one complete, independent library per environment (Lua 5.1-5.5, standalone Luau, Roblox Luau).

## Key features

- **Scope tree.** A full lexical scoping model with Module, Function, Block, and Loop scope kinds.
- **Reference classification.** The builder records every identifier use as Read, Write, or ReadWrite.
- **Shadowing detection.** The tree flags declarations that reuse a name from an outer scope.
- **Upvalue tracking.** Captures across function boundaries carry their source and destination scopes.
- **Per-environment stdlib.** Typed signatures with overloads, deprecation metadata down to individual parameters and constant values, purity and must-use markers, and value shapes for method resolution.
- **Conservative shape resolution.** `resolve.rs` maps chains like `local f = io.open(...)` and `game:GetService("Players")` to stdlib entries without a type-inference engine.

## Architecture

### Scope tree

The tree models lexical scoping with four scope kinds:

- **Module** is the top-level scope of a file.
- **Function** is a function body. Names captured from outer scopes become upvalues.
- **Block** covers `do...end`, `then`, and `else` blocks.
- **Loop** covers `for`, `while`, and `repeat` bodies.

Each scope owns its locally declared symbols and tracks every reference to them.

### Symbols and references

A `Symbol` is a declared variable with a `SymbolKind` (`Local`, `Parameter`, `IteratorVariable`, `NumericForVariable`, `FunctionName`) and the span of its declaration. A `Reference` is a usage site with a `ReferenceKind` (Read, Write, or ReadWrite; the last covers `x += 1` in Luau and similar compound assignments).

Names not declared in any enclosing scope are unresolved. They reach the global scope, where they may match a stdlib entry or remain unknown.

### Standard library

Each environment is one fully self-contained TOML file under `stdlib_data/`, selected by `(LuaVersion, StdlibEnvironment)`:

- `lua51.toml` through `lua55.toml`, one per numbered Lua version, verified against that version's reference manual.
- `luau.toml`, standalone (open-source) Luau.
- `luau_roblox.toml`, the Roblox runtime, plus two **generated** files spliced in at load: `roblox_api.toml` (service and class-name constant sets from the Roblox API dump) and `roblox_enums.toml` (the full `Enum` tree). Regenerate both with `cargo test -p luck_semantic regenerate_roblox_api -- --ignored`; never hand-edit them.

Both Luau catalogs include the distinct `integer` primitive's library, the matching `buffer.readinteger` / `buffer.writeinteger` APIs, and the current Luau math predicates and constants (`isnan`, `isinf`, `isfinite`, `nan`, `e`, `phi`, `sqrt2`, `tau`).

The files are deliberately independent, with no inheritance or tier layering. Shared entries are duplicated, and the drift-guard suite in `tests/drift.rs` keeps them honest by cross-checking every entry shared between files against an explicit allowlist of manually verified divergences.

Entries model:

- **Overloaded signatures.** Typed parameters with per-signature arity (`CFrame.new`, `collectgarbage`).
- **Shapes.** Named member sets for non-global values (`file`, the derived `string` receiver, `Instance`, `DataModel`, `EnumItem`, ...) with `extends` composition. Entries declare the shape they return, so method chains resolve.
- **Constant parameters.** Closed string-value sets (`game:GetService` service names, `Instance.new` class names, `collectgarbage` options), shareable across parameters via named `constant_sets`.
- **Deprecation.** At entry, parameter, and constant-value level, with `%n` replace templates for auto-fixes.
- **Purity / must-use / read-only** markers.

### Module layout

| File | Purpose |
|------|---------|
| `lib.rs` | Public API and `analyze()` entry point |
| `scope.rs` | Scope tree data structures |
| `builder.rs` | AST visitor that constructs the scope tree |
| `nodes.rs` | Flat pre-order node table (`Nodes`/`collect_nodes`) with parent links and per-node scope, shared with the linter |
| `stdlib_model.rs` | Stdlib data model, queries, and the per-environment library instances |
| `stdlib_load.rs` | TOML deserialization and composition into the `StdlibLibrary` model |
| `resolve.rs` | Conservative shape and callee resolution |
| `stdlib_data/` | Per-environment library data (+ generated Roblox data) |
