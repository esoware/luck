# luck_ast

The syntax tree of Lua 5.1 to 5.5 and Luau, and what walks, measures, and builds it.

## Overview

- A tree lives in a `luck_allocator` arena. Every node is in an arena `Box` or `List`, none needs dropping, and the tree is freed with its arena at once. A tree is `Send` and `Sync`.
- A `Block` holds `Statement`s and an optional `LastStatement`. Every value is an `Expression`, and a Luau type annotation is a `Type`, a tree of its own rather than text.
- A leaf, a `Name` or a `Literal`, holds its text as written: a parsed tree's borrows the source, and a built tree's lives in the arena, so a printer never needs the source. A `Name` also holds a `NameIndex`, which the parser numbers and a pass that holds the tree mutably may renumber.
- A run of suffixes (`a.b[c]:d()`) is one `SuffixedExpression`, and a run of binary operators at one precedence (`a + b - c`) one `BinaryOp`, so a run nests one level however long it is.
- Comments aren't in the tree. The lexer keeps them in a list of their own.

## Usage

- `Visitor` walks a tree and `VisitorMut` rewrites one in place. Override the `visit_*` hooks you need and call `self.walk_*` from them to keep recursing.
- `Synth` builds nodes without source, for a tool that emits a tree. It parenthesizes, escapes, and spells numbers so the printed tree reads back as the same tree. `SpanCounter` gives its nodes distinct spans, and a `SyntheticComment` anchors a comment to one for the formatter.
- `MAX_NESTING_DEPTH` is the deepest tree `luck_parser` builds, and `MIN_STACK_BYTES` the stack a thread needs to run any luck stage on a tree that deep, since every traversal recurses once per level. `block_nesting_depth` and `expr_nesting_depth` measure a tree built some other way, safely at any depth.
- `TreeCounts` is what a parsed tree holds, which `luck_semantic` sizes its tables from.

```rust
use luck_allocator::Allocator;
use luck_ast::{Call, Visitor};
use luck_token::LuaVersion;

struct CallCounter(usize);

impl<'ast> Visitor<'ast> for CallCounter {
    fn visit_call(&mut self, call: Call<'ast>) {
        self.0 += 1;
        self.walk_call(call);
    }
}

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, "f(g(1))", LuaVersion::Lua54);
let mut counter = CallCounter(0);
counter.visit_block(&parsed.block);
assert_eq!(counter.0, 2);
```
