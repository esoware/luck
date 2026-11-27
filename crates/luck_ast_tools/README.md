# luck_ast_tools

Generates `luck_ast`'s visitors and trait implementations from its node types.

After changing a type that derives `Ast`, run `just ast` to rewrite the files
in `luck_ast/src/generated/`. A test fails while any of them is out of date.
The tool reads the type definitions as source text, so it still runs when
`luck_ast` doesn't compile. Ported from oxc's `tasks/ast_tools`.
