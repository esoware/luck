# luck_semantic

Semantic analysis of Luau, in one walk over a tree, as `oxc_semantic` does it.

## Overview

`SemanticBuilder` walks a chunk's `Block` once, in source order, with Luau's scoping rules, and builds a `Semantic`. Its `Scoping` is always built: flat tables of scopes, symbols, and references, indexed by typed ids, and the chunk's distinct names, which it owns, so a pass that rewrites the tree can keep it with `Semantic::into_scoping`. A method's `self`, type functions, and type-only scopes all bind as Luau binds them.

Every name in the tree finds what it declares or refers to by its `NameIndex`, which the parser gives each name of its own, so a lookup is one index into a table of one word per name, with no search. Resolving a name hashes it once: the chunk's names are interned by their length and their first and last eight bytes, which tell apart every name of up to 16 bytes, so finding one compares words and reads no text. Each other part is built only when the caller asks for it:

- `with_check` reports the errors Luau makes beyond the grammar, each an E008 `luck_diagnostics::Diagnostic`: a write to a `const` local or function, an `export const`, or an `export function`, a type function that reads a local from outside it, and an `until` that reads a local a `continue` skips. It checks no limit on locals or upvalues, since Luau's compiler counts what its optimizer leaves, which depends on the level it compiles at.
- `with_nodes` builds `AstNodes`, every node of the tree that has an `AstKind`, in source preorder, with its id, its parent, and the scope it's in, and the set of the node types it holds, as `oxc_semantic`'s does. Symbols, references, and scopes point into it by `NodeId`, which every build numbers the same way whether or not it builds the table.
- `with_requires` finds every `RequireSite`, a call of the host's `require`, by its name or through the global table as `_G.require`, with its module path when it's a string literal.

Nothing here knows a variant's standard library. `luck_stdlib` reads a chunk's semantic against one.

## Usage

- `SemanticBuilder::new()`, then `with_counts`, the parse's `TreeCounts`, which sizes every table, and the parts wanted, then `build(chunk)`, which returns a `SemanticBuilderReturn` of the `Semantic` and the checker's errors in source order.
- `Scoping`: `scopes()`, `symbols()`, and `references()`, with `Scope`, `Symbol`, and `Reference` and their ids. `symbol_of` and `reference_of` take a tree's `Name` to what it is, `Reference::resolved` holds the symbol a reference reaches, as `resolved_symbol_of` gives it for a name, and `resolved_references` lists a symbol's references, which `retain_resolved_references` prunes for a pass that drops the names that made some of them. A reference also says whether it's in a type, which never runs, and whether it's a `local function`'s to itself from its own body. `references_within` lists the references in a span by position, from an index it sorts the first time it's asked, since only it reads one.
- `AstNodes`: `iter()`, `get(id)`, `kind(id)`, `parent_node(id)`, `ancestor_ids(id)`, `node_at(offset)`, the innermost node at an offset, from which a tool such as an editor walks up through the parents, and `contains_any(types)`, which takes an `AstTypesBitset`, as a lint rule's node types are.

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
