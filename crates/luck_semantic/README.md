# luck_semantic

Scope, symbol, and reference analysis for Luau.

`SemanticBuilder` walks a chunk once, in source order, and builds its
`Scoping`, which holds every scope, symbol, and reference in flat tables
indexed by typed ids. Each name in the tree carries a `NameIndex`, which
`Scoping::symbol_of` and `Scoping::reference_of` turn into what the name
declares or refers to. The design follows `oxc_semantic`.

The rest is opt-in. `with_check` reports code that the grammar accepts but Luau
rejects, such as an assignment to a `const` local. `with_nodes` builds an
`AstNodes` table with every node's parent and scope, and `with_requires`
collects every `require` call.

No standard library is involved, so every name that isn't declared is a global.
`luck_stdlib` adds a library on top.

## Usage

This example assigns to a `const` local, then finds both of its references and
the error:

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

* `SemanticBuilder`, which runs the walk with `build`, or with `rebuild` to
  reuse an earlier result's memory, and returns a `SemanticBuilderReturn`.
* `Semantic`, which holds the `Scoping`, along with the `AstNodes` and
  `RequireSite`s when they were asked for.
* `Scoping`, with its `Scope`, `Symbol`, and `Reference` entries and their
  `ScopeKind`, `SymbolKind`, and `ReferenceKind`.
* `ScopeId`, `SymbolId`, `ReferenceId`, `NameId`, and `NodeId`, the typed ids.
* `AstNodes` and `AstNode`, every node in preorder with its parent and scope.
* `AstTypesBitset`, the set of node types a tree contains.
* `RequireSite`, one call of `require`.
