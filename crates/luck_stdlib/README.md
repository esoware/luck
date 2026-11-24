# luck_stdlib

The standard library of each target luck reads, Lua 5.1 to 5.5, Luau, and Roblox, and what a chunk's names and calls reach in it.

## Overview

Each target has a static catalog built into the crate, with signatures, overloads, value shapes, and deprecations. `just stdlib` generates the catalogs from `stdlib_data/`, checking the data as it goes, so nothing is parsed or validated at runtime, and `just roblox-api` refreshes the Roblox services, classes, and enums from the live API dump first.

`analyze` builds a chunk's `luck_semantic::Semantic`, with its node table and `require` sites, and reads it against its target's catalog, so a caller can ask whether a name is a known global or which stdlib function a call reaches. A local initialized from a value of known shape, such as `local f = io.open(...)`, has that shape, so `f:read()` resolves too. It lives apart from `luck_semantic`, so a pass that needs only scoping, such as the minifier's, builds no catalog.

## Usage

- `library_for(target)` returns the catalog, a `StdlibLibrary`.
- `analyze(chunk, counts, target)` returns a `SemanticAnalysis` of a tree parsed as `target`'s version, and beside it the semantic checker's errors. `SemanticAnalysis` answers `is_known_global`, `resolve_callee`, and `resolve_stdlib_path`, takes the configured extra globals with `add_extra_globals`, and holds the `Semantic`.

```rust
use luck_allocator::Allocator;
use luck_token::LuaTarget;

let allocator = Allocator::new();
let target = LuaTarget::Lua54;
let source = b"local greeting = 1 print(greeting)";
let parsed = luck_parser::parse(&allocator, source, target.version());
let chunk = allocator.alloc(parsed.chunk);
let (analysis, errors) = luck_stdlib::analyze(chunk, parsed.counts, target);
assert!(errors.is_empty());
let scoping = analysis.scoping();
let greeting = scoping.symbols()[0].id;
assert_eq!(scoping.resolved_references(greeting).len(), 1);
assert!(analysis.is_known_global("print"));
```
