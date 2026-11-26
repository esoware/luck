# luck_ast_tools

Generates `luck_ast`'s visitors, node kinds, and trait implementations from its node types.

Run `just ast` after changing a type that derives `Ast`. It rewrites these files in `luck_ast/src/generated/`:

- `visit.rs` and `visit_mut.rs` for `Visit` and `VisitMut`
- `ast_kind.rs` for `AstKind` and `AstType`
- `derive_get_span.rs` for `GetSpan`
- `derive_content_eq.rs` for `ContentEq`
- `ast_builder.rs` for each node type's constructors
- `assert_layouts.rs` for a test that pins every type's size

A test fails if any of them is out of date. Ported from oxc's `tasks/ast_tools`.
