# luck_ast

The syntax tree of Lua 5.1 to 5.5 and Luau, and what walks, measures, and builds it.

## Overview

- A tree lives in a `luck_allocator` arena. Every node is in an arena `Box` or `List`, none needs dropping, and the tree is freed with its arena at once. A tree is `Send` and `Sync`.
- A `Block` holds `Statement`s and an optional `LastStatement`. Every value is an `Expression`, and a Luau type annotation is a `Type`, a tree of its own rather than text.
- A leaf, a `Name` or a `Literal`, holds its text as written. A parsed leaf borrows it from the source and a built one keeps it in the arena, so a printer never needs the source. A `Name` also holds a `NameIndex`, which the parser numbers and a pass that holds the tree mutably may renumber.
- A run of suffixes (`a.b[c]:d()`) is one `SuffixedExpression`, and a run of binary operators at one precedence (`a + b - c`) one `BinaryOp`, so a run nests one level however long it is.
- Comments aren't in the tree. The lexer keeps them in a list of their own.
- Every type in the tree derives `luck_ast_macros::Ast`, and `luck_ast_tools` writes `src/generated/` from those types. Run `just ast` after changing one.

## Usage

- `Visit` walks a tree and `VisitMut` rewrites one in place. Each has a `visit_*` hook for every type, which defaults to the `walk_*` method that visits what the node holds in source order. Override the hooks you need and call `self.walk_*` from them to keep recursing. `enter_node` and `leave_node` see every node, as an `AstKind` in `Visit` and an `AstType` in `VisitMut`.
- `GetSpan` gives every node's span, and `ContentEq` compares two trees with their spans left out, so `a==1` and `a == 1` hold the same tree.
- Every node type has a `new` constructor, a `boxed` one when a field holds it in a `Box`, and an enum a `new_*` one for each variant that holds a struct, as `Expression::new_binary_op`. They check nothing.
- `Synth` builds nodes without source on top of those, for a tool that emits a tree. It parenthesizes, escapes, and spells numbers so the printed tree reads back as the same tree. `SpanCounter` gives its nodes distinct spans, and a `SyntheticComment` anchors a comment to one for the formatter.
- `MAX_NESTING_DEPTH` is the deepest tree `luck_parser` builds, and `MIN_STACK_BYTES` the stack a thread needs to run any luck stage on a tree that deep, since every traversal recurses once per level. `block_nesting_depth` and `expr_nesting_depth` measure a tree built some other way, safely at any depth.
- `TreeCounts` counts what a parsed tree holds, and `luck_semantic` sizes its tables from it.

```rust
use luck_allocator::Allocator;
use luck_ast::{SuffixedExpression, Visit};
use luck_token::LuaVersion;

struct CallCounter(usize);

impl<'ast> Visit<'ast> for CallCounter {
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.0 += suffixed_expr.calls().count();
        self.walk_suffixed_expr(suffixed_expr);
    }
}

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, "f(g(1))", LuaVersion::Lua54);
let mut counter = CallCounter(0);
counter.visit_block(&parsed.block);
assert_eq!(counter.0, 2);
```
