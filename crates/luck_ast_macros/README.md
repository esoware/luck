# luck_ast_macros

`#[derive(Ast)]`, which marks a type as part of `luck_ast`'s syntax tree and generates nothing. `luck_ast_tools` reads the marked types and writes the tree's support code from them, and the derive lets the types carry the tool's attributes:

- `#[visit(level)]` on a type makes its walk one level of nesting, and on a field makes visiting that field one.
- `#[content_eq(skip)]` on a field leaves it out of `ContentEq`, for a field that records only how the source was written.

```rust,ignore
#[derive(Debug, PartialEq, Eq, Ast)]
#[visit(level)]
pub struct TableConstructor<'ast> {
    pub span: Span,
    pub fields: List<'ast, Field<'ast>>,
    #[content_eq(skip)]
    pub has_trailing_separator: bool,
}
```
