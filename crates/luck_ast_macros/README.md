# luck_ast_macros

The `#[derive(Ast)]` marker that `luck_ast`'s node types use for code generation.

The derive itself generates nothing. It marks a type for `luck_ast_tools` and lets it carry these attributes:

- `#[visit(level)]` counts walking into this type or field as one level of nesting.
- `#[visit(no_kind)]` means this struct isn't a node of its own and has no `AstKind`.
- `#[content_eq(skip)]` leaves this field out of `ContentEq`.

Ported from `oxc_ast_macros`.
