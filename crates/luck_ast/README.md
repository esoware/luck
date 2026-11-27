# luck_ast

A syntax tree for Luau, with visitors and a builder for making trees in code.

A tree is a `Block` of `Statement`s, with an `Expression` for each value and a
`Type` for each annotation. Nodes live in a `luck_allocator` arena and nothing
in the tree needs dropping, so a whole tree is freed with its arena. Names and
literals carry their own text and decoded values, which means a printer never
has to look at the original source.

Some shapes are flattened to keep trees shallow. A chain of suffixes such as
`a.b[c]:d()` is one `SuffixedExpression`, and a run of operators at one
precedence level, such as `a + b - c`, is one `BinaryOp`. The tree keeps only
the parentheses that change meaning, and `Expression::needs_parens` tells a
printer where the rest go.

## Usage

This example collects every name in a chunk with a `Visit` implementation that
overrides only the hook for names:

```rust
use luck_allocator::Allocator;
use luck_ast::{Name, Visit};

struct Names<'ast>(Vec<&'ast str>);

impl<'ast> Visit<'ast> for Names<'ast> {
    fn visit_name(&mut self, name: &'ast Name<'ast>) {
        self.0.push(name.text.as_str());
    }
}

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"local total = count + 1");
let mut names = Names(Vec::new());
names.visit_block(&parsed.chunk);
assert_eq!(names.0, ["total", "count"]);
```

`VisitMut` works the same way but rewrites the tree in place, and `Synth`
builds new trees from scratch.

## Stack depth

Every walk recurses once per level of nesting. `luck_parser` never builds a
tree deeper than `MAX_NESTING_DEPTH`, and a thread with `MIN_STACK_BYTES` of
stack can walk any tree that deep. A tree built with `Synth` has no such cap,
so measure it with `block_nesting_depth` before walking it.

## API

* `Block`, `Statement`, `Expression`, and `Type`, the roots, with a struct for
  each kind of statement, expression, and type, such as `IfStatement`,
  `BinaryOp`, and `FunctionType`.
* `Name`, `NumberLiteral`, and `StringLiteral`, the leaves. Each `Name` holds a
  `NameIndex`, which semantic analysis uses to find what the name refers to.
* `Declaration`, `DeclaredGlobal`, `DeclaredFunction`, `DeclaredExternType`,
  and `ExternTypeMember`, the `declare` statements of a definition file.
* `Visit` and `VisitMut`, which walk a tree, and `AstKind`, `AstType`, and
  `AST_TYPE_MAX`, which name any node.
* `GetSpan`, which gives any node's span, and `ContentEq`, which compares two
  trees while ignoring spans.
* `Synth`, which builds trees, with `SpanCounter`, `FieldSpec`,
  `TypeFieldSpec`, `FunctionSignature`, and `InterpolatedStringPart`.
* `Slot`, `Call`, `ChainHead`, `DottedPath`, and `TreeCounts`, which answer
  common questions about a tree.
* `MAX_NESTING_DEPTH`, `MIN_STACK_BYTES`, `block_nesting_depth`,
  `expr_nesting_depth`, and `spans_past_nesting_cap`, for nesting depth.
* `is_valid_identifier` and `sanitize_identifier`, which check and fix Luau
  names.
* `OneOrMore`, a list that's never empty.
