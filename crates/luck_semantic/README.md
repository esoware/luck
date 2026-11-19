# luck_semantic

Scope analysis and standard library knowledge for Lua and Luau.

## Overview

`ScopeTree::build` walks a tree once, in evaluation order and with the target version's scoping rules, into flat tables of scopes, symbols, and references indexed by typed ids. It also numbers the chunk's distinct names, borrowed from the tree, so a fact about a name is a table index. A `NodeSink` sees every node the walk reaches, so a consumer such as the linter builds its own view of the tree in the same walk.

`analyze` reads that tree against one environment's standard library. Each of Lua 5.1 to 5.5, Luau, and Roblox has a static catalog built into the crate, with signatures, overloads, value shapes, and deprecations, so a caller can ask whether a name is a known global or which stdlib function a call reaches.

## Usage

- `analyze(chunk, counts, environment)` returns a `SemanticAnalysis` of a tree in the version its `Chunk` records, and `analyze_with` shows a `NodeSink` the walk. `SemanticAnalysis` answers `is_known_global` and `resolve_callee`, and holds the `ScopeTree`.
- `ScopeTree`: `scopes()`, `symbols()`, and `references()`, with `Scope`, `Symbol`, and `Reference` and their ids. `reference_of` takes a tree's `Name` to its reference, `Reference::resolved` holds the symbol it reaches, and `resolved_references` lists a symbol's references. `build_for_names` records only the names a caller asks about.
- `library_for(version, environment)` returns the catalog, a `StdlibLibrary`.

```rust
use luck_allocator::Allocator;
use luck_token::{LuaVersion, StdlibEnvironment};

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, "local greeting = 1 print(greeting)", LuaVersion::Lua54);
let chunk = allocator.alloc(parsed.chunk);
let analysis = luck_semantic::analyze(chunk, parsed.counts, StdlibEnvironment::Standalone);
let tree = analysis.scope_tree();
let greeting = tree.symbols()[0].id;
assert_eq!(tree.resolved_references(greeting).len(), 1);
assert!(analysis.is_known_global("print"));
```
