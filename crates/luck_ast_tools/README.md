# luck_ast_tools

Internal and unpublished: writes `luck_ast`'s support code from the types in it that derive `Ast`, into `luck_ast/src/generated/`. Ported from oxc's `tasks/ast_tools`.

```sh
just ast
```

| File | What it holds |
| --- | --- |
| `visit.rs`, `visit_mut.rs` | `Visit` and `VisitMut`, with a `visit_*` hook and a `walk_*` method for every type the tree visits |
| `ast_kind.rs` | `AstKind`, a reference to any node, and `AstType`, which kind it is |
| `derive_get_span.rs` | `GetSpan` for every node |
| `derive_content_eq.rs` | `ContentEq` for every type |
| `ast_builder.rs` | `new`, `boxed`, and `new_<variant>` constructors for every node type |
| `assert_layouts.rs` | A test that pins every type's size and alignment against a snapshot |

The tool parses `luck_ast`'s source with `syn` rather than depending on it, so it runs however broken the tree's definitions leave the crate. It formats what it writes with `rustfmt` and the workspace's `rustfmt.toml`, so `cargo fmt` leaves the files as they are, and the `generated_code_is_up_to_date` test fails while one is stale.

A type that derives `Ast` takes two of the tool's attributes:

- `#[visit(level)]` on a struct makes its walk a level of nesting, and on a field makes visiting that field one. The levels are the ones `luck_parser` counts against `MAX_NESTING_DEPTH`.
- `#[content_eq(skip)]` on a field leaves it out of `ContentEq`, which also leaves out every `Span`.

A node with no `span` field spans its fields, so it needs one that's always there and no list.
