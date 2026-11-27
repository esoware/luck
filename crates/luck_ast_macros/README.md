# luck_ast_macros

The `#[derive(Ast)]` marker for `luck_ast`'s node types.

The derive generates nothing. It marks the types that `luck_ast_tools` reads to
generate the tree's visitors and trait implementations, and lets them carry
these attributes:

* `#[visit(level)]` - Walking into this type or field counts as one level of
  nesting.
* `#[visit(no_kind)]` - This struct isn't a node of its own, and gets no
  `AstKind`.
* `#[content_eq(skip)]` - `ContentEq` ignores this field.

Ported from `oxc_ast_macros`.
