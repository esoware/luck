# luck_bundler

Dependency graph construction and single-file bundling for Lua/Luau projects.

## Overview

The bundler takes a multi-file Lua project and produces one self-contained output file. It walks the dependency graph from an entry module, resolves every `require()` through `luck_resolver`, and registers each module with a small memoizing loader that mirrors the target version's own `require` implementation, so modules load lazily, on first require, with real `require` semantics.

## Key Features

- **Position-independent requires** — a `require("string_literal")` resolves from anywhere: any statement, any expression position, any function body. The lazy loader makes require order irrelevant, so bare side-effect requires and requires below other code all bundle. A non-string-literal argument (`require(name)`) cannot name a module at build time, so it is kept and resolved at runtime instead of failing the build - see **Dynamic requires**. Require strings are decoded like real Lua string literals, so `require("a\46b")` and `require("a.b")` are the same module.
- **Dynamic requires degrade, they do not abort** — `require(expr)` is governed by `dynamic_require` in `luck.json` (or `--dynamic-require`): `warn` (the default) keeps the call and reports W007, `allow` keeps it silently, `error` restores the old hard failure with E002. On Lua targets the kept call is retargeted at the loader's `__luck_dynamic`, which serves any name registered in the bundle - the cache is keyed by module name, so a runtime-computed name can hit it - and otherwise falls through to the host's own `require`, read at call time. Luau keys its cache by resolved file and a Roblox `require` takes an Instance, so there the call is left exactly as written.
- **Scope-aware rewriting** — only calls that actually reach the global `require` are bundled. A call through a local binding named `require` keeps its user semantics (W005 makes it visible), and aliasing the global (`local r = require`) warns that those call sites escape bundling.
- **Version-exact loader** — the emitted loader reproduces the target's `ll_require` observable behavior. Lua targets key the cache by module name and back it with `package.loaded` itself when available: preseeded entries win, `package.preload` beats bundled files, a module returning `false` reloads on the next require, a module returning nil caches as `true`, and two require strings reaching one file execute it twice, each under its own name. 5.1 keeps the "loop or previous error" sentinel; 5.2+ retries failed loads; 5.2+ chunks receive `(name, path)`; 5.4+ returns the loader data as a second result on first load. Luau uses a private file-keyed cache, calls chunks with no arguments, raises on load-time cycles, and errors unless a module returns exactly one non-nil value.
- **Cycles bundle with a warning** — cycles emit W003 (with the full cycle path); deferred cycles (mutual requires inside function bodies) work exactly as in real Lua, and a load-time cycle fails at runtime the same way the target version fails. A cycle through the entry works too: the entry's function registers under its require string, so a module requiring the entry re-executes it as a fresh instance, exactly like real Lua loading the main file a second time as a module.
- **Entry wrapper** — the entry body is wrapped in a function invoked with the chunk's varargs (`return __luck_entry(...)`), so CLI args and chunk returns flow through unchanged and the entry keeps its full 200-local budget.
- **Collision-proof identifiers** — every generated name shares one prefix chosen to appear nowhere in any module source (`__luck_`, falling back to `__luck1_`, `__luck2_`, ...), so user code can never capture or shadow loader internals.
- **No 200-locals ceiling** — modules live as table slots, not one local per module, so bundles scale past Lua's 200-locals-per-function limit.
- **No host paths in the output** — every path the bundle carries (module provenance comments and the file path 5.2+ chunks receive as loader data) is project-relative. A module above the project root keeps its shape through `..` segments, so the root's own side of the tree - the part holding the build host's home and project directories - is never named; when a module shares no root with the project at all (another Windows drive or UNC share) only its file name is emitted.
- **Zero-runtime output** — the loader is plain inline Lua; no external helper library and no monkey-patched `require`.

## Architecture

### Pipeline

1. **Require extraction** — `require_extraction.rs` walks each module's entire AST for `require()` calls, using `luck_semantic` scope analysis to skip calls that resolve to local bindings, and decodes each string literal to its runtime value. It produces a list of require sites (decoded string plus spans), the callee spans of dynamic requires the emitter retargets, and validation diagnostics (W007/E002 non-string-literal argument, E006 `package.loaded` write on Luau, W001 duplicate require, W005 shadowed/aliased require).

2. **Graph construction** — `graph.rs` starts at the entry file and does a breadth-first walk, resolving each require through `luck_resolver` and enqueuing newly discovered modules. A `GraphBuilder` owns the whole in-progress graph (modules, id/index maps, the `petgraph` graph, the resolver, and the diagnostic buffers), so discovery is a set of methods rather than one function threading a dozen scratch buffers. Read/parse/resolve failures surface as E008/E009/E010/E011/E012; inert hot comments in non-entry Luau modules surface as W006.

3. **Topological sort** — the standard `petgraph` toposort orders roots first; the bundler reverses the result so leaves come first (cosmetic only; the loader does not depend on it). A cycle falls back to discovery order and records W003.

4. **Loader emission** — `emitter.rs` picks the collision-free identifier prefix, emits the version-specific loader, and registers modules. Lua targets get one slot per unique require string, keyed by name like `package.loaded`; strings reaching the same file share its function but keep separate cache entries. Luau targets get one numeric slot per resolved file. The entry body becomes `local __luck_entry = function(...) … end`, registered under its require strings when something requires it, and invoked last with the chunk's varargs. Every resolved `require()` is spliced over with a loader call that preserves the call's newline count, and the emitter produces a line map (`LineMapEntry`: `bundle_start_line`/`bundle_end_line`/`path`) that stays 1:1 so runtime tracebacks map back to source files.

### Module Identity

`module.rs` defines `ModuleId` (an opaque index into the graph's module list), `Dependency` (a resolved require edge: decoded require string, resolved path, and call span), and `ModuleInfo`, which carries a module's path, source text, discovered dependencies, project-relative path, the callee spans of its dynamic requires, and an optional cached parsed `Block` (populated during graph construction to avoid re-parsing in the emitter).

`ModuleInfo::path` is the canonical absolute path and is the graph's identity key; `ModuleInfo::relative_path` (built by `graph::make_relative` against the project root, which resolves to the working directory when the caller has none) is the only one the emitted bundle is allowed to name. `BundleResult::source_files` and the `LineMapEntry` paths stay canonical: they are local build/debug data, not part of the shipped file.

### Span-Based Rewriting

Require rewriting splices over the exact byte ranges the dependency scan recorded, so the extractor and the emitter can never disagree about which calls are bundled. Even unusually formatted requires — multi-line calls, long-bracket arguments, escaped strings, Luau type casts — survive bundling, while `require(...)` text inside string literals is left untouched and shadowed calls stay exactly as written.

When a Luau value-export module is placed inside a generated loader wrapper, the emitter lowers its top-level `export` declarations to local declarations and appends the frozen export table that the module would otherwise return implicitly. Type-only exports are made private inside wrappers.

### Testing

Inline unit tests cover individual modules; `tests/integration.rs` bundles fixture projects end-to-end and asserts on the emitted output via `insta` snapshots under `tests/snapshots/`.
