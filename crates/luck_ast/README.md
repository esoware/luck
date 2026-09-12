# luck_ast

Abstract syntax tree definitions and traversal infrastructure for Lua 5.1-5.5 and Luau.

## Overview

`luck_ast` defines the syntactic shape every luck tool reads or writes. The AST follows Lua's grammar closely while staying compact enough for whole-program transforms. Compile-time tests hold `Expression` and `Statement` to a 64-byte budget each.

## Key features

- **Complete language coverage.** Lua 5.1 through 5.5, plus the Luau extensions: exact integer literals, explicit type instantiation, value exports, if-expressions, interpolated strings, type casts, compound assignment, attributes, and generalized iteration.
- **Luau type AST.** A `Type` node family (`luck_ast::types`) covering named/generic references, table types, function types, unions/intersections, optionals, singletons, `typeof`, type packs, and variadic/generic packs. Type annotations, `TypeCast`, `TypeDeclaration`, and `FunctionBody` generics/return types hold real `Type` nodes rather than opaque spans.
- **64-byte enum budget.** `Expression`, `Statement`, and `Type` are each <=64 bytes, with large variants boxed. Size tests in `lib.rs` fail the build if a variant grows past the budget.
- **Exhaustive matching.** Neither enum is `#[non_exhaustive]`. Every transform and visitor must cover every variant, so adding a new node makes the compiler point at every site that needs updating.
- **`Visitor` and `AstTransform` traits.** One for read-only analysis, one for ownership-based rewrites. Both own recursion through `walk_*` defaults, including `walk_type` for the type grammar; consumers override only the cases they care about.
- **Programmatic synthesis.** The `synth` module (`luck_ast::synth::Synth`) builds AST nodes without any source text, handing out fresh monotonic dummy spans. `Synth::share` clones a synthesizer onto the same counter, so several of them, including ones on different threads, feed one tree without span collisions; `Synth::starting_at` offsets the range when splicing into a parsed AST. Constructors take `&self`, so calls nest. `binop`/`unop`/`type_cast` parenthesize operands by operator precedence, prefix positions auto-wrap non-prefix expressions, and the string and number constructors handle escaping and literal-less values (negatives, infinities, NaN, `i64::MIN`, non-UTF-8 byte strings). It targets tools that emit an AST directly and carries `SyntheticComment` for node-anchored comment attachment. The node types carry data only. There are no fluent `with_*` constructors; `span()` accessors live in `span.rs`, and `Punctuated<T>` helpers sit alongside the type in `shared.rs`.

## Architecture

### Core types

A `Block` is the fundamental unit: a sequence of `Statement`s followed by an optional `LastStatement`. Every function body, loop body, and `do...end` contains one.

`Expression` covers literals (`Nil`, `False`, `True`, `Number`, Luau `Integer`, `StringLiteral`, `VarArg`), compound forms (`BinaryOp`, `UnaryOp`, `Parenthesized`, `TableConstructor`, `FunctionDef`, `FunctionCall`, `Var`), and Luau extensions (`IfExpression`, `InterpolatedString`, `TypeCast`, `TypeInstantiation`).

`Statement` covers the imperative side: `Assignment`, `FunctionCall`, `DoBlock`, `WhileLoop`, `RepeatLoop`, `IfStatement`, `NumericFor`, `GenericFor`, `FunctionDecl`, `LocalFunction`, `LocalAssignment`, plus the version-gated statements `Goto` / `Label` (5.2+), attribute-bearing `LocalAssignment` (5.4+), `GlobalDeclaration` / `GlobalFunction` / `GlobalStar` (5.5), `CompoundAssignment` and `TypeDeclaration` (Luau). Flags on the declaration nodes mark exported Luau locals and functions.

`LastStatement` has four variants: `Return`, `Break`, `Continue` (Luau), `Error`.

### Shared types

- **`Punctuated<T>`** is a comma-separated list: a `Vec<T>` plus a `has_trailing_separator` flag. Context implies separator spelling and position, so the type stores no separator tokens or spans. Argument lists, variable lists, and field lists all use it.
- **`FunctionBody`** holds parameters, body block, and optional return type annotation, plus an optional Luau generic list (`<T, U...>`), shared across the four function-bearing variants.
- **`Parameter`** is a name with an optional Luau type annotation. It covers both function parameters and generic-for loop bindings; the trailing `...` rides in a separately boxed `VarArgParam`.
- **Optional payload layout.** `FunctionBody.vararg` is `Option<Box<VarArgParam>>`, and `AttributedName.attrib` is `Option<Box<Attribute>>`. This reduces the common attribute-free, non-vararg AST footprint, but each present payload needs an allocation. Vararg-heavy and attribute-heavy programs can request more total bytes than inline storage. `FnSig` and the synthesis constructors still accept unboxed values and box them when constructing AST nodes.
- **`Field`** is a table constructor entry: keyed (`[expr] = expr`), named (`name = expr`), or positional (`expr`).

### Types

The Luau type grammar lives in `types.rs` as its own `Type` node family, held to the same 64-byte budget as `Expression` and `Statement`. `Type` covers named and generic references (`Name`, `module.Name`, `Name<args>`), table types, function types, unions and intersections, postfix optionals (`T?`), literal singletons, `typeof(expr)`, parenthesized types, explicit type packs (`(T, U)`), and variadic (`...T`) / generic (`T...`) pack elements. `Visitor` and `AstTransform` carry matching `visit_type` / `transform_type` arms (with `walk_type` recursion), and the `synth` module builds these nodes too. Type nodes appear only in Luau sources; the parser gates them on `LuaVersion` feature predicates.

### Traversal

`Visitor` walks the AST read-only by borrowing each node. Override `visit_expression` or `visit_statement`, then call `self.walk_expression` / `self.walk_statement` to recurse. Linter rules and scope analysis live here.

`AstTransform` takes each node by value and returns a replacement. Override `transform_expression` or `transform_statement`, then call `self.walk_*` to apply default recursion before or after your rewrite. Every minifier pass uses this pattern.

The difference is ownership. `Visitor` borrows, so you read but cannot modify; `AstTransform` consumes, so you can restructure, replace, or remove nodes.

### Node discriminants and structural queries

`node.rs` defines `NodeType` (one variant per `Statement`/`LastStatement`/`Expression` variant, built by the exhaustive `of_stmt`/`of_last_stmt`/`of_expr`), a borrowed `NodeKind` view, and `AstTypesBitset`, a fixed-size bitset over `NodeType` used by `luck_semantic`'s flat node table and `luck_linter`'s node-type-bucketed rule dispatch. `query.rs` holds small read-only structural predicates shared by the codegen and formatter printers, such as whether a statement's or expression's first emitted token is `(` or `{`. Both printers have to guard those cases against reprinting into a different parse.
