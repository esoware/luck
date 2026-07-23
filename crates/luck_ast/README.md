# luck_ast

Abstract syntax tree definitions and traversal infrastructure for Lua 5.1–5.5 and Luau.

## Overview

`luck_ast` defines the syntactic shape every luck tool reads or writes. The AST follows Lua's grammar closely while staying compact enough for whole-program transforms: `Expression` and `Statement` are each kept under a 64-byte budget, enforced by compile-time tests.

## Key Features

- **Complete language coverage** — Lua 5.1 through 5.5, plus Luau extensions including exact integer literals, explicit type instantiation, value exports, if-expressions, interpolated strings, type casts, compound assignment, attributes, and generalized iteration.
- **Luau type AST** — a full `Type` node family (`luck_ast::types`) covering named/generic references, table types, function types, unions/intersections, negation, optionals, singletons, `typeof`, type packs, and variadic/generic packs. Type annotations, `TypeCast`, `TypeDeclaration`, and `FunctionBody` generics/return types hold real `Type` nodes rather than opaque spans.
- **64-byte enum budget** — `Expression`, `Statement`, and `Type` are each ≤64 bytes by boxing large variants. Size tests in `lib.rs` fail the build if a variant grows past the budget.
- **Exhaustive matching** — neither enum is `#[non_exhaustive]`. Every transform and visitor must cover every variant, so adding a new node surfaces every site that needs updating.
- **Visitor and AstTransform traits** — one for read-only analysis, one for ownership-based rewrites. Both own recursion through `walk_*` defaults (including `walk_type` for the type grammar); consumers override only the cases they care about.
- **Programmatic synthesis** — the `synth` module (`luck_ast::synth::Synth`) builds AST nodes without any source text, handing out fresh monotonic dummy spans (`Synth::share` clones a synthesizer onto the same counter so several — including across threads — feed one tree without span collisions; `Synth::starting_at` offsets the range when splicing into a parsed AST). Constructors take `&self`, so calls nest; `binop`/`unop`/`type_cast` parenthesize operands by operator precedence, prefix positions auto-wrap non-prefix expressions, and string/number constructors handle escaping and literal-less values (negatives, infinities, NaN, `i64::MIN`, non-UTF-8 byte strings). It targets tools that emit an AST directly and carries `SyntheticComment` for node-anchored comment attachment. The node types carry data only — there are no fluent `with_*` constructors; `span()` accessors live in `span.rs` and `Punctuated<T>` helpers alongside the type in `shared.rs`.

## Architecture

### Core Types

A `Block` is the fundamental unit: a sequence of `Statement`s followed by an optional `LastStatement`. Every function body, loop body, and `do…end` contains one.

`Expression` covers literals (`Nil`, `False`, `True`, `Number`, Luau `Integer`, `StringLiteral`, `VarArg`), compound forms (`BinaryOp`, `UnaryOp`, `Parenthesized`, `TableConstructor`, `FunctionDef`, `FunctionCall`, `Var`), and Luau extensions (`IfExpression`, `InterpolatedString`, `TypeCast`, `TypeInstantiation`).

`Statement` covers the imperative side: `Assignment`, `FunctionCall`, `DoBlock`, `WhileLoop`, `RepeatLoop`, `IfStatement`, `NumericFor`, `GenericFor`, `FunctionDecl`, `LocalFunction`, `LocalAssignment`, plus version-gated statements — `Goto` / `Label` (5.2+), attribute-bearing `LocalAssignment` (5.4), `GlobalDeclaration` / `GlobalFunction` / `GlobalStar` (5.5), `CompoundAssignment` and `TypeDeclaration` (Luau). Exported Luau locals/functions are represented by flags on their declaration nodes.

`LastStatement` has four variants: `Return`, `Break`, `Continue` (Luau), `Error`.

### Shared Types

- **`Punctuated<T>`** is a comma-separated list — a `Vec<T>` plus a `has_trailing_separator` flag. Separator spelling and position are implied by context, so no separator tokens or spans are stored. Used for argument lists, variable lists, and field lists.
- **`FunctionBody`** holds parameters, body block, and optional return type annotation, plus an optional Luau generic list (`<T, U...>`), shared across the four function-bearing variants.
- **`Parameter`** is a typed name — a name with an optional Luau type annotation. It is reused both for function parameters and generic-for loop bindings; the trailing `...` rides in a separate `VarArgParam`.
- **`Field`** is a table constructor entry — keyed (`[expr] = expr`), named (`name = expr`), or positional (`expr`).

### Types

The Luau type grammar lives in `types.rs` as its own `Type` node family, held to the same ≤64-byte budget as `Expression` and `Statement`. `Type` covers named and generic references (`Name`, `module.Name`, `Name<args>`), table types, function types, unions and intersections, negation (`~T`), postfix optionals (`T?`), literal singletons, `typeof(expr)`, parenthesized types, explicit type packs (`(T, U)`), and variadic (`...T`) / generic (`T...`) pack elements. `Visitor` and `AstTransform` carry matching `visit_type` / `transform_type` arms (with `walk_type` recursion), and the `synth` module builds these nodes too. Type nodes appear only in Luau sources — the parser gates them on `LuaVersion` feature predicates.

### Traversal

`Visitor` walks the AST read-only by borrowing each node. Override `visit_expression` or `visit_statement`, then call `self.walk_expression` / `self.walk_statement` to recurse. Linter rules and scope analysis live here.

`AstTransform` takes each node by value and returns a replacement. Override `transform_expression` or `transform_statement`, then call `self.walk_*` to apply default recursion before or after your rewrite. Every minifier pass uses this pattern.

The key difference: `Visitor` borrows, so you read but cannot modify; `AstTransform` consumes, so you can restructure, replace, or remove nodes.
