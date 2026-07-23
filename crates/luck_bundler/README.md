# luck_bundler

Dependency graph construction and single-file bundling for Lua/Luau projects.

## Overview

The bundler takes a multi-file Lua project and produces one self-contained output file. It walks the dependency graph from an entry module, resolves every `require()` through `luck_resolver`, and registers each non-entry module with a small memoizing loader so modules load lazily, on first require, with real `require` semantics.

## Key Features

- **Position-independent requires** — a `require("string_literal")` resolves from anywhere: any statement, any expression position, any function body. The lazy loader makes require order irrelevant, so bare side-effect requires and requires below other code all bundle. Only a non-string-literal argument (`require(name)`) is rejected, with E002.
- **Cycles bundle with a warning** — the lazy loader is registration-order independent, so a dependency cycle no longer blocks bundling. Cycles emit W003 (with the full cycle path); deferred cycles (mutual requires inside function bodies) work exactly as in real Lua, and a load-time cycle raises at runtime with a clear loader error.
- **Topological emission** — modules are emitted leaves-first so every dependency is registered before its consumer (cosmetic only; the loader does not depend on it).
- **Lazy memoizing loader** — a small loader (`__luck_require`) is emitted once at the top of every multi-module bundle. Each non-entry module registers as `__luck_modules[id] = function(...) … end` and loads on first require; results cache (a module returning nil caches as `true`, like `package.loaded`), and a cycle hit while a module is still loading raises at runtime. The entry module is inlined directly. Require call sites are rewritten to `__luck_require(id)`, and the whole output is wrapped in `do…end` to keep the loader locals out of global scope. A single-module bundle is emitted verbatim, with no loader.
- **No 200-locals ceiling** — modules live as numbered table slots, not one local per module, so bundles scale past Lua's 200-locals-per-function limit.
- **Zero-runtime output** — the loader is plain inline Lua; no external helper library and no monkey-patched `require`.

## Architecture

### Pipeline

1. **Require extraction** — `require_extraction.rs` walks each module's entire AST for `require()` calls and validates their form. It produces a list of require sites (string plus spans) and a list of validation diagnostics (E002 non-string-literal argument, E006 `package.loaded` write, W001 duplicate require).

2. **Graph construction** — `graph.rs` starts at the entry file and does a breadth-first walk, resolving each require through `luck_resolver` and enqueuing newly discovered modules. A `GraphBuilder` owns the whole in-progress graph (modules, id/index maps, the `petgraph` graph, the resolver, and the diagnostic buffers), so discovery is a set of methods rather than one function threading a dozen scratch buffers. Read/parse/resolve failures surface as E008/E009/E010/E011/E012.

3. **Topological sort** — the standard `petgraph` toposort orders roots first; the bundler reverses the result so leaves come first. A cycle falls back to discovery order and records W003.

4. **Loader emission** — `emitter.rs` walks the sorted modules. Each non-entry module is assigned a numeric slot and registered as `__luck_modules[slot] = function(...) … end`; the entry module's body is inlined last (no wrapper). Every `require()` that resolved to a bundled module is rewritten to `__luck_require(slot)`. The whole output is wrapped in `do…end`. The emitter also produces a line map (`LineMapEntry`: `bundle_start_line`/`bundle_end_line`/`path`) so runtime tracebacks map back to source files.

### Module Identity

`module.rs` defines `ModuleId` (an opaque index into the graph's module list), `Dependency` (a resolved require edge: require string, resolved path, and call span), and `ModuleInfo`, which carries a module's path, source text, discovered dependencies, sanitized `__luck_`-prefixed name, and an optional cached parsed `Block` (populated during graph construction to avoid re-parsing in the emitter).

### AST-Based Rewriting

Require rewriting happens on the AST, not on source text: the emitter walks the parsed block, collects the span of every `require("...")` that resolved to a bundled module, and splices `__luck_require(slot)` into the source at those spans. This means even unusually formatted requires — multi-line strings, long-bracket arguments, Luau type casts — survive bundling, while `require(...)` text inside string literals is left untouched.

When a Luau value-export module is placed inside a generated loader wrapper, the emitter lowers its top-level `export` declarations to local declarations and appends the frozen export table that the module would otherwise return implicitly. Type-only exports are made private inside wrappers.
