# luck_ast

The Luau syntax tree every luck tool works on, with visitors and a builder for making trees in code.

## Overview

A tree's root is a `Block`, which is a list of `Statement`s. Values are `Expression`s and type annotations are `Type`s.

Trees live in a `luck_allocator` arena and are freed along with it. Names and literals carry their own text, so printing a tree never needs the source. `Synth` builds trees in code.

A chain like `a.b[c]:d()` is one `SuffixedExpression`, and `a + b - c` is one `BinaryOp`, so long chains stay one level deep.

The tree keeps only the parentheses that change meaning. `Expression::needs_parens` tells a printer where the rest go.

`Visit` reads a tree and `VisitMut` edits it in place. `ContentEq` compares two trees, ignoring spans.

`luck_ast_tools` generates both visitors, `AstKind`, `GetSpan`, `ContentEq`, and the node constructors.

## Stack depth

Every walk recurses once per level of nesting. `luck_parser` never returns a tree deeper than `MAX_NESTING_DEPTH`, and walking one that deep takes `MIN_STACK_BYTES` of stack. Run luck on threads that have at least that.

A tree from `Synth` has no depth limit. Check it with `block_nesting_depth` before walking it.

## Usage

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

## API

- Roots: `Block`, `Statement`, `Expression`, `Type`
- Statements: `LocalAssignment`, `Assignment`, `CompoundAssignment`, `IfStatement`, `WhileLoop`, `RepeatLoop`, `NumericFor`, `GenericFor`, `DoBlock`, `FunctionDecl`, `LocalFunction`, `ReturnStatement`, `TypeDeclaration`, and their parts
- Expressions: `BinaryOp`, `UnaryOp`, `SuffixedExpression`, `FunctionDef`, `TableConstructor`, `IfExpression`, `InterpolatedString`, `TypeCast`, `ParenthesizedExpression`, and their parts
- Types: `NamedType`, `FunctionType`, `TableType`, `UnionType`, `IntersectionType`, `OptionalType`, `TypeofType`, `TypePack`, and their parts
- Leaves: `Name`, `NameIndex`, `NumberLiteral`, `StringLiteral`
- Definition files: `DeclaredGlobal`, `DeclaredFunction`, `DeclaredExternType`, `ExternTypeMember`
- Traversal: `Visit`, `VisitMut`, `AstKind`, `AstType`, `AST_TYPE_MAX`
- Queries: `GetSpan`, `ContentEq`, `Slot`, `Call`, `ChainHead`, `DottedPath`, `TreeCounts`
- Building: `Synth`, `SpanCounter`, `FieldSpec`, `TypeFieldSpec`, `FunctionSignature`, `InterpolatedStringPart`
- Depth: `MAX_NESTING_DEPTH`, `MIN_STACK_BYTES`, `block_nesting_depth`, `expr_nesting_depth`, `type_nesting_depth`, `spans_past_nesting_cap`
- Identifiers: `is_valid_identifier`, `sanitize_identifier`
- Containers: `OneOrMore`
