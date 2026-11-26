# luck_semantic

Scope, symbol, and reference analysis for Luau, in a single pass over the syntax tree.

## Overview

`SemanticBuilder` does the walk. Each name in the tree finds its symbol or reference through its `NameIndex`. Based on `oxc_semantic`.

Three more parts are off unless you ask for them:

- `with_check` reports code the grammar allows but Luau rejects, such as assigning to a `const`.
- `with_nodes` builds `AstNodes`, every node with its parent and scope.
- `with_requires` collects every `require` call.

It knows nothing about the standard library. `luck_stdlib` adds that.

## Usage

```rust
use luck_allocator::Allocator;
use luck_semantic::SemanticBuilder;

let allocator = Allocator::new();
let source = b"const greeting = 1 greeting = print(greeting)";
let parsed = luck_parser::parse(&allocator, source);
let chunk = allocator.alloc(parsed.chunk);
let built = SemanticBuilder::new().with_counts(parsed.counts).with_check(true).build(chunk);

let scoping = built.semantic.scoping();
let greeting = scoping.symbols()[0].id;
assert_eq!(scoping.resolved_references(greeting).len(), 2);
assert_eq!(built.errors[0].message, "attempt to assign to const variable `greeting`");
```

## API

- `SemanticBuilder` configures the walk, then runs it with `build`, or with `rebuild` to reuse an earlier result's memory.
- `SemanticBuilderReturn` holds the `Semantic` and the checker's errors.
- `Semantic` gives the `scoping`, `nodes`, and `requires`.
- `Scoping` holds every scope, symbol, reference, and name. Look them up with `symbol_of`, `reference_of`, and `resolved_references`.
- `Scope`, `ScopeKind`, `Symbol`, `SymbolKind`, `Reference`, and `ReferenceKind` are its entries.
- `ScopeId`, `SymbolId`, `ReferenceId`, `NameId`, and `NodeId` index them.
- `AstNodes` lists every node in preorder as an `AstNode`, with its parent and scope.
- `AstTypesBitset` records which node types a tree contains.
- `RequireSite` is one `require` call.
