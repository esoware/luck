# luck_ast

The syntax tree of Luau, and what walks, measures, and builds it.

## Overview

- A tree lives in a `luck_allocator` arena. Every node is in an arena `Box` or `List`, none needs dropping, and the tree is freed with its arena at once. A tree is `Send` and `Sync`.
- A tree's root is a `Chunk`, which holds its `Block`, a list of `Statement`s. A `return`, a `break`, and a `continue` are statements like any other, and `Statement::has_to_end_block` picks them out as the ones the grammar takes only at the end of a block. A `;` is no node. Every value is an `Expression`, and a type annotation is a `Type`, a tree of its own rather than text.
- A `Name` holds its text, and a literal, a `NumberLiteral`, `IntegerLiteral`, or `StringLiteral`, its value, which the parser decodes once, beside its spelling as written, which a built literal leaves out. A string's value is bytes, a number's the `f64` it reads as, and an integer literal's, such as `123i`, an `i64`. A parsed leaf borrows from the source and a built one lives in the arena, so a printer never needs the source. A `Name` also holds a `NameIndex`, which the parser numbers, each name its own, and by which `luck_semantic` finds what the name declares or refers to.
- A run of suffixes (`a.b[c]:d()`) is one `SuffixedExpression`, and a run of binary operators at one precedence (`a + b - c`) one `BinaryOp`, so a run nests one level however long it is.
- The tree keeps a `ParenthesizedExpression` only where the parentheses change what they hold: a call or `...` they truncate to one value, or an integer literal they keep from reading as negative under `-`. A printer asks `Expression::needs_parens` whether an expression needs a pair in the `Slot` it writes it in, as `oxc_codegen` derives them, and writes a run held in a run at its own level in parentheses.
- A part most nodes of a kind lack, an `if`'s `else` or a type annotation, is boxed, so a node without one doesn't hold its space.
- Comments aren't in the tree. The lexer keeps them in a list of their own.
- Every type in the tree derives `luck_ast_macros::Ast`, and `luck_ast_tools` writes `src/generated/` from those types. Run `just ast` after changing one.

## Usage

- `Visit` walks a tree and `VisitMut` rewrites one in place. Each has a `visit_*` hook for every type, which defaults to the `walk_*` method that visits what the node holds in source order. Override the hooks you need and call `self.walk_*` from them to keep recursing. `enter_node` and `leave_node` see every node, as an `AstKind` in `Visit` and an `AstType` in `VisitMut`.
- `GetSpan` gives every node's span, and `ContentEq` compares two trees with their spans and spellings left out, so `a==1` and `a == 1` hold the same tree, as `"a"` and `'a'` do.
- Every node type has a `new` constructor, a `boxed` one when a field holds it in a `Box`, and an enum a `new_*` one for each variant that holds a struct, as `Expression::new_binary_op`. They check nothing.
- `Synth` builds nodes without source on top of those, for a tool that emits a tree. It checks names, builds literals from values, which the printers spell, joins runs and chains, and puts a statement that has to end its block but doesn't in a `do ... end` of its own, so the printed tree reads back as the same tree. `SpanCounter` gives its nodes distinct spans, which a `luck_formatter::SyntheticComment` anchors a comment to.
- `MAX_NESTING_DEPTH` is the deepest tree `luck_parser` builds, and `MIN_STACK_BYTES` the stack a thread needs to run any luck stage on a tree that deep, since every traversal recurses once per level. `block_nesting_depth` and `expr_nesting_depth` measure a tree built some other way, safely at any depth.
- `TreeCounts` counts what a parsed tree holds, and `luck_semantic` sizes its tables from it.

```rust
use luck_allocator::Allocator;
use luck_ast::{SuffixedExpression, Visit};

struct CallCounter(usize);

impl<'ast> Visit<'ast> for CallCounter {
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        self.0 += suffixed_expr.calls().count();
        self.walk_suffixed_expr(suffixed_expr);
    }
}

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"f(g(1))");
let mut counter = CallCounter(0);
counter.visit_chunk(&parsed.chunk);
assert_eq!(counter.0, 2);
```
