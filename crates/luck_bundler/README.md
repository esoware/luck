# luck_bundler

Dependency graph construction and single-file bundling for Lua/Luau projects.

## Overview

The bundler takes a multi-file Lua project and produces one self-contained output file. It walks the dependency graph from an entry module, resolves every `require()` through `luck_resolver`, and registers each module with a small memoizing loader that mirrors the target version's own `require` implementation. Modules load lazily, on first require, with real `require` semantics.

## Key features

- **Position-independent requires.** A `require("string_literal")` resolves from anywhere: any statement, any expression position, any function body. The lazy loader makes require order irrelevant, so bare side-effect requires and requires below other code all bundle. The one rejected form is a non-string-literal argument (`require(name)`), which reports E002. The extractor decodes require strings like real Lua string literals, so `require("a\46b")` and `require("a.b")` are the same module.
- **Scope-aware rewriting.** Only calls that reach the global `require` get bundled. A call through a local binding named `require` keeps its user semantics (W005 makes it visible), and aliasing the global (`local r = require`) warns that those call sites escape bundling.
- **Version-exact loader.** The emitted loader reproduces the target's `ll_require` observable behavior. Lua targets key the cache by module name and back it with `package.loaded` itself when available: preseeded entries win, `package.preload` beats bundled files, a module returning `false` reloads on the next require, a module returning nil caches as `true`, and two require strings reaching one file execute it twice, each under its own name. 5.1 keeps the "loop or previous error" sentinel; 5.2+ retries failed loads; 5.2+ chunks receive `(name, path)`; 5.4+ returns the loader data as a second result on first load. Luau uses a private file-keyed cache, calls chunks with no arguments, raises on load-time cycles, and errors unless a module returns exactly one non-nil value.
- **Cycles bundle with a warning.** A cycle emits W003 with the full cycle path. Deferred cycles (mutual requires inside function bodies) work exactly as in real Lua, and a load-time cycle fails at runtime the same way the target version fails. A cycle through the entry works too: the entry's function registers under its require string, so a module requiring the entry re-executes it as a fresh instance, exactly like real Lua loading the main file a second time as a module.
- **Entry wrapper.** The emitter wraps the entry body in a function invoked with the chunk's varargs (`return __luck_entry(...)`), so CLI args and chunk returns flow through unchanged and the entry keeps its full 200-local budget.
- **Collision-proof identifiers.** Every generated name shares one prefix chosen to appear nowhere in any module source (`__luck_`, falling back to `__luck1_`, `__luck2_`, ...), so user code can never capture or shadow loader internals.
- **No 200-locals ceiling.** Modules live as table slots, not one local per module, so bundles scale past Lua's 200-locals-per-function limit.
- **Zero-runtime output.** The loader is plain inline Lua, with no external helper library and no monkey-patched `require`.

## Architecture

### Pipeline

1. **Require extraction.** `require_extraction.rs` walks each module's entire AST for `require()` calls, using `luck_semantic` scope analysis to skip calls that resolve to local bindings, and decodes each string literal to its runtime value. It produces a list of require sites (decoded string plus spans) and validation diagnostics (E002 non-string-literal argument, E006 `package.loaded` write on Luau, W001 duplicate require, W005 shadowed/aliased require).

2. **Graph construction.** `graph.rs` starts at the entry file and does a breadth-first walk, resolving each require through `luck_resolver` and enqueuing newly discovered modules. A `GraphBuilder` owns the whole in-progress graph (modules, id/index maps, the `petgraph` graph, the resolver, and the diagnostic buffers), so discovery is a set of methods rather than one function threading a dozen scratch buffers. Read, parse, and resolve failures report E008/E009/E010/E011/E012; an inert hot comment in a non-entry Luau module reports W006.

3. **Topological sort.** The standard `petgraph` toposort orders roots first; the bundler reverses the result so leaves come first (cosmetic only; the loader does not depend on it). A cycle falls back to discovery order and records W003.

4. **Loader emission.** `emitter.rs` picks the collision-free identifier prefix, emits the version-specific loader, and registers modules. Lua targets get one slot per unique require string, keyed by name like `package.loaded`; strings reaching the same file share its function but keep separate cache entries. Luau targets get one numeric slot per resolved file. The entry body becomes `local __luck_entry = function(...) ... end`, registered under its require strings when something requires it, and invoked last with the chunk's varargs. The emitter splices a loader call over every resolved `require()`, preserving the call's newline count, and produces a line map (`LineMapEntry`: `bundle_start_line`/`bundle_end_line`/`path`) that stays 1:1 so runtime tracebacks map back to source files.

### Module identity

`module.rs` defines `ModuleId` (an opaque index into the graph's module list), `Dependency` (a resolved require edge: decoded require string, resolved path, and call span), and `ModuleInfo`, which carries a module's path, source text, discovered dependencies, sanitized name, project-relative path (the loader data 5.2+ chunks receive), and an optional cached parsed `Block` (populated during graph construction to avoid re-parsing in the emitter).

### Span-based rewriting

Require rewriting splices over the exact byte ranges the dependency scan recorded, so the extractor and the emitter can never disagree about which calls are bundled. Unusually formatted requires survive bundling too: multi-line calls, long-bracket arguments, escaped strings, Luau type casts. `require(...)` text inside string literals stays untouched, and shadowed calls stay exactly as written.

When a Luau value-export module goes inside a generated loader wrapper, the emitter lowers its top-level `export` declarations to local declarations and appends the frozen export table the module would otherwise return implicitly. Type-only exports become private inside wrappers.

### Testing

Inline unit tests cover individual modules; `tests/integration.rs` bundles fixture projects end-to-end and asserts on the emitted output via `insta` snapshots under `tests/snapshots/`.
