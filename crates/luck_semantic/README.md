# luck_semantic

Semantic analysis of Lua 5.1 to 5.5 and Luau, in one walk over a tree, as `oxc_semantic` does it.

## Overview

`SemanticBuilder` walks a `Chunk` once, in source order, with its version's scoping rules, and builds a `Semantic`. Its `Scoping` is always built: flat tables of scopes, symbols, references, and labels, indexed by typed ids, and the chunk's distinct names, which it owns, so a pass that rewrites the tree can keep it with `Semantic::into_scoping`. Lua 5.1's implicit `arg` in a vararg function, a method's `self`, a local `_ENV`, Lua 5.5's `global` declarations, and Luau's type functions and type-only scopes all bind as their compilers bind them.

Every name in the tree finds what it declares, refers to, or labels by its `NameIndex`, which the parser gives each name of its own, so a lookup is one index into a table, with no search. Each other part is built only when the caller asks for it:

- `with_check` reports the errors Lua's compiler and Luau's parser make beyond the grammar, each an E008 `luck_diagnostics::Diagnostic`: a write to a `<const>`, `<close>`, or other read-only binding, a free name no Lua 5.5 `global` declaration covers, a `goto` no label takes or one that jumps into the scope of a local, a duplicate label, a Luau type function that reads a local from outside it, and a Luau `until` that reads a local a `continue` skips. In Lua 5.1 to 5.5 it reports the function that holds more than 200 active locals or more upvalues than its version allows, as `lparser.c` counts them while it parses, with the hidden locals of a `for` loop, and in Lua 5.4+ leaving out what a compile-time constant takes no register or upvalue for. Luau's compiler counts what its optimizer leaves, which depends on the level it compiles at, so it has no limits checked.
- `with_nodes` builds `AstNodes`, every node of the tree that has an `AstKind`, in source preorder, with its id, its parent, and the scope it's in, and the set of the node types it holds, as `oxc_semantic`'s does. Symbols, references, labels, and scopes point into it by `NodeId`, which every build numbers the same way whether or not it builds the table.
- `with_requires` finds every `RequireSite`, a call of the host's `require`, by its name or through the global table as `_G.require`, with its module path when it's a string literal.

Nothing here knows a target's standard library. `luck_stdlib` reads a chunk's semantic against one.

## Usage

- `SemanticBuilder::new()`, then `with_counts`, the parse's `TreeCounts`, which sizes every table, and the parts wanted, then `build(chunk)`, which returns a `SemanticBuilderReturn` of the `Semantic` and the checker's errors in source order.
- `Scoping`: `scopes()`, `symbols()`, `references()`, and `labels()`, with `Scope`, `Symbol`, `Reference`, and `Label` and their ids. A function's or the chunk's `Scope` says whether its own code reads `...`. `symbol_of`, `reference_of`, and `label_of` take a tree's `Name` to what it is, `Reference::resolved` holds the symbol a reference reaches, and `resolved_references` lists a symbol's references, which `retain_resolved_references` prunes for a pass that drops the names that made some of them. A reference also says whether it's in a Luau type, which never runs, and whether it's a `local function`'s to itself from its own body. `declared_globals` lists where each Lua 5.5 `global` statement declares a name. `references_within` lists the references in a span by position.
- `AstNodes`: `iter()`, `get(id)`, `kind(id)`, `parent_node(id)`, `ancestor_ids(id)`, `node_at(offset)`, the innermost node at an offset, from which a tool such as an editor walks up through the parents, and `contains_any(types)`, which takes an `AstTypesBitset`, as a lint rule's node types are.

```rust
use luck_allocator::Allocator;
use luck_semantic::SemanticBuilder;
use luck_token::LuaVersion;

let allocator = Allocator::new();
let source = b"local greeting <const> = 1 greeting = print(greeting)";
let parsed = luck_parser::parse(&allocator, source, LuaVersion::Lua54);
let chunk = allocator.alloc(parsed.chunk);
let built = SemanticBuilder::new().with_counts(parsed.counts).with_check(true).build(chunk);
let scoping = built.semantic.scoping();
let greeting = scoping.symbols()[0].id;
assert_eq!(scoping.resolved_references(greeting).len(), 2);
assert_eq!(built.errors[0].message, "attempt to assign to const variable `greeting`");
```
