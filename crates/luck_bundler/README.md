# luck_bundler

Dependency graph construction and single-file bundling for Lua/Luau projects.

## Overview

The bundler takes a multi-file Lua project and produces one self-contained output file. It walks the dependency graph from an entry module, resolves every `require()` through `luck_resolver`, and registers each non-entry module with a small memoizing loader so modules load lazily, on first require, with real `require` semantics.

## Key Features

- **Strict require form** — only `local x = require("string_literal")` is accepted. Bare `require()`, computed arguments, or assignments to non-locals are rejected with specific diagnostic codes.
- **Cycle detection** — circular dependencies are reported as E005 with the full cycle path.
- **Topological emission** — modules are emitted leaves-first so every dependency is defined before its consumer.
- **Lazy memoizing loader** — a small loader (`__luck_require`) is emitted once at the top of every multi-module bundle. Each non-entry module registers as `__luck_modules[id] = function(...) … end` and loads on first require; results cache (a module returning nil caches as `true`, like `package.loaded`), and a cycle hit while a module is still loading raises at runtime. The entry module is inlined directly. Require call sites are rewritten to `__luck_require(id)`, and the whole output is wrapped in `do…end` to keep the loader locals out of global scope. A single-module bundle is emitted verbatim, with no loader.
- **No 200-locals ceiling** — modules live as numbered table slots, not one local per module, so bundles scale past Lua's 200-locals-per-function limit.
- **Zero-runtime output** — the loader is plain inline Lua; no external helper library and no monkey-patched `require`.

## Architecture

### Pipeline

1. **Require extraction** — `require_extraction.rs` scans each module's top-level statements for `require()` calls and validates their form. It produces a list of resolved require sites and a list of validation diagnostics (E001 require after non-require, E002 non-string-literal argument, E003 bare require, E006 `package.loaded` write, W001 duplicate require, W002 top-level vararg).

2. **Graph construction** — `graph.rs` starts at the entry file, resolves each require through `luck_resolver`, and walks the dependency graph depth-first. The result is a `petgraph` directed graph with cycle detection. Cycles fail with E005.

3. **Topological sort** — the standard `petgraph` toposort orders roots first; the bundler reverses the result so leaves come first. This guarantees a dependency module is defined before any module that requires it.

4. **Loader emission** — `emitter.rs` walks the sorted modules. Each non-entry module is assigned a numeric slot and registered as `__luck_modules[slot] = function(...) … end`; the entry module's body is inlined last (no wrapper). Every `require()` that resolved to a bundled module is rewritten to `__luck_require(slot)`. The whole output is wrapped in `do…end`. The emitter also produces a line map (`LineMapEntry`: `bundle_start_line`/`bundle_end_line`/`path`) so runtime tracebacks map back to source files.

### Module Identity

`module.rs` defines `ModuleId` (an opaque index into the graph's module list) and `ModuleInfo`, which carries a module's path, source text, discovered dependencies, sanitized `__luck_`-prefixed name, and an optional cached parsed `Block` (populated during graph construction to avoid re-parsing in the emitter).

### AST-Based Rewriting

Require rewriting happens on the AST, not on source text: the emitter walks the parsed block, collects the span of every `require("...")` that resolved to a bundled module, and splices `__luck_require(slot)` into the source at those spans. This means even unusually formatted requires — multi-line strings, long-bracket arguments, Luau type casts — survive bundling, while `require(...)` text inside string literals is left untouched.
