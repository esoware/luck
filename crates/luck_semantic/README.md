# luck_semantic

Scope analysis, symbol resolution, and per-environment standard library definitions for Lua and Luau.

## Overview

`luck_semantic` builds a `ScopeTree` from a parsed AST, tracking variable declarations, references, shadowing, and upvalue captures across function boundaries. It also ships the stdlib catalog the linter and LSP use to reason about built-ins: one complete, independent library per environment (Lua 5.1-5.5, standalone Luau, Roblox Luau).

## Key features

- **Scope tree.** Lexical scopes of four kinds: Module, Function, Block, and Loop.
- **Reference classification.** The builder records every identifier use as Read, Write, or ReadWrite.
- **Shadowing detection.** Each declaration links to the visible binding of the same name that it hides, whether that binding is in an outer scope or the same one.
- **Upvalue tracking.** Locals referenced from inside a nested function are flagged as upvalues.
- **Per-environment stdlib.** Typed signatures with overloads, deprecation metadata down to individual parameters and constant values, purity and must-use markers, and value shapes for method resolution.
- **Conservative shape resolution.** `resolve.rs` maps chains like `local f = io.open(...)` and `game:GetService("Players")` to stdlib entries without a type-inference engine.

## Architecture

### Scope tree

The tree models lexical scoping with four scope kinds:

- **Module** is the top-level scope of a file.
- **Function** is a function body. Names captured from outer scopes become upvalues.
- **Block** covers `do...end`, `then`, and `else` blocks.
- **Loop** covers `for`, `while`, and `repeat` bodies.

Each scope records its kind and its parent, and each symbol records its scope and lists its references.

### Symbols and references

A `Symbol` is a declared variable with a `SymbolKind` (`Local`, `Parameter`, `IteratorVariable`, `NumericForVariable`, `FunctionName`, `ImplicitSelf`) and the span of its declaration, which is empty for the implicit `self`. A `Reference` is a usage site with a `ReferenceKind`: Read, Write, or ReadWrite, the last for Luau compound assignments such as `x += 1`.

A name with no visible declaration is a free name and stays unresolved. Under a Lua 5.2+ local `_ENV`, or in a chunk that assigns its own `_ENV`, it's a table field and not a global, so `ScopeTree::unresolved_references` leaves it out. Inside a Luau `type function`, the `types` library and the type declarations in scope belong to the type runtime and get no reference.

A free name that is a global may be a stdlib entry, a configured extra global, a name a Lua 5.5 `global` statement declares (`ScopeTree::declared_globals`), or unknown. `ScopeTree::reaches_host_global` answers the question the stdlib rules ask: does this identifier still read the host's value? A name the chunk writes anywhere (`ScopeTree::assigned_globals`) doesn't.

### Standard library

Each environment has its own TOML file under `stdlib_data/`, selected by `(LuaVersion, StdlibEnvironment)`:

- `lua51.toml` through `lua55.toml`, one per numbered Lua version, verified against that version's reference manual.
- `luau.toml`, standalone (open-source) Luau.
- `luau_roblox.toml`, the Roblox runtime, plus two **generated** files spliced in at load: `roblox_api.toml` (service and class-name constant sets from the Roblox API dump) and `roblox_enums.toml` (the full `Enum` tree). Regenerate both with `cargo test -p luck_semantic regenerate_roblox_api -- --ignored`. Never hand-edit them.

Both Luau catalogs include the library for Luau's `integer` type, `buffer.readinteger` and `buffer.writeinteger`, and the newer math functions and constants (`isnan`, `isinf`, `isfinite`, `nan`, `e`, `phi`, `sqrt2`, `tau`).

The files don't inherit from each other, so shared entries are duplicated. `tests/it/drift.rs` requires the two Luau files, and each pair of adjacent Lua versions, to agree on every entry they share unless an allowlist names the difference.

Entries model:

- **Overloaded signatures.** Typed parameters with per-signature arity (`CFrame.new`, `collectgarbage`).
- **Shapes.** Named member sets for kinds of values (`file`, the derived `string` receiver, `Instance`, `DataModel`, `EnumItem`, ...), and a shape can name a base shape with `extends`. Functions declare the shape they return and values the shape they have, so method chains resolve.
- **Constant parameters.** Closed string-value sets (`game:GetService` service names, `Instance.new` class names, `collectgarbage` options), shareable across parameters via named `constant_sets`.
- **Deprecation.** At entry, parameter, and constant-value level, with `%n` replace templates for auto-fixes. A placeholder before a call's `)` forwards the remaining arguments, and an operand placeholder groups a compound argument. A deprecation carries no template when none can keep the behavior (`table.maxn`, most of `bit32`).
- **Purity / must-use / read-only** markers.

### Module layout

| File | Purpose |
|------|---------|
| `lib.rs` | Crate docs and the flat public surface |
| `analysis.rs` | `SemanticAnalysis`, the `analyze()` entry points, and its scope and stdlib queries |
| `scope.rs` | Scope tree data structures |
| `builder.rs` | AST visitor that constructs the scope tree, exported as `build_scope_tree` for callers that need no stdlib, and lists each local's initializer for the shape step |
| `resolve.rs` | Conservative shape and callee resolution, each read forward from a chain's prefix, `ChainCallees` for a chain's calls in one pass, and the shapes of initialized locals |
| `stdlib_model.rs` | Stdlib data model and its queries |
| `stdlib_load.rs` | TOML deserialization, composition into the `StdlibLibrary` model, and the per-environment library instances |
| `stdlib_data/` | Per-environment library data (+ generated Roblox data) |
