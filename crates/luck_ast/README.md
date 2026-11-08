# luck_ast

Abstract syntax tree definitions and traversal infrastructure for Lua 5.1-5.5 and Luau.

## Overview

`luck_ast` defines the syntax tree every luck tool reads or writes. It follows Lua's grammar closely while staying compact enough for whole-program transforms.

## Key features

- **Complete language coverage.** Lua 5.1 through 5.5, plus the Luau extensions: exact integer literals, explicit type instantiation, value exports, if-expressions, interpolated strings, type casts, compound assignment, attributes, and generalized iteration.
- **Luau type AST.** A `Type` node family covering named/generic references, table types, function types, unions/intersections, optionals, singletons, `typeof`, type packs, and variadic/generic packs. Type annotations, `TypeCast`, `TypeDeclaration`, and `FunctionBody` generics and return types hold real `Type` nodes, not opaque spans.
- **64-byte enum budget.** `Expression`, `Statement`, and `Type` are each at most 64 bytes, with large variants boxed. Unit tests in `layout.rs` enforce the budget and pin the exact size of every node, so any node that grows fails the test suite.
- **Exhaustive matching.** No node enum is `#[non_exhaustive]`, and transforms and visitors match without catch-all arms, so adding a new node makes the compiler point at every site that needs updating.
- **`Visitor` and `Transform` traits.** One for read-only analysis, one for ownership-based rewrites. Each hook defaults to a `walk_*` recursion, including `walk_type` for the type grammar, so consumers override only the hooks they care about.
- **Programmatic synthesis.** `Synth` (`synth.rs`, its constructors split by node family under `synth/`) builds AST nodes without any source text, handing out fresh, increasing point spans. `Synth::share` gives another synthesizer on the same counter, so several of them, even on different threads, can feed one tree without span collisions. `Synth::starting_at` starts the counter at a given offset, so nodes spliced into a parsed AST can sit above its byte offsets. Constructors take `&self`, so calls nest. `binary_op`, `unary_op`, and `type_cast` parenthesize operands by precedence, prefix positions wrap expressions that can't stand there, and the string and number constructors handle escaping and values with no literal form (negatives, infinities, NaN, `i64::MIN`, non-UTF-8 byte strings). It's meant for tools that emit an AST directly, and `SyntheticComment` carries comments, leading or trailing like a lexed `Comment`, for the formatter to attach to nodes. The node types themselves have no builder methods. Their `span()` accessors live in `span.rs`, and the `Punctuated<T>` helpers sit beside the type in `punctuated.rs`.

## Architecture

### Core types

A `Block` is the fundamental unit: a sequence of `Statement`s followed by an optional `LastStatement`. Every function body, loop body, and `do...end` contains one.

`Expression` covers literals (`Nil`, `False`, `True`, `Number`, Luau `Integer`, `StringLiteral`, `VarArg`), compound forms (`BinaryOp`, `UnaryOp`, `Parenthesized`, `TableConstructor`, `FunctionDef`, `FunctionCall`, `Var`), and Luau extensions (`IfExpression`, `InterpolatedString`, `TypeCast`, `TypeInstantiation`).

`Statement` covers the imperative side: `Assignment`, `FunctionCall`, `DoBlock`, `WhileLoop`, `RepeatLoop`, `IfStatement`, `NumericFor`, `GenericFor`, `FunctionDecl`, `LocalFunction`, `LocalAssignment`, plus the version-gated statements `Goto` / `Label` and mid-block `Break` (5.2+), attribute-bearing `LocalAssignment` (5.4+), `GlobalDeclaration` / `GlobalFunction` / `GlobalStar` (5.5), `CompoundAssignment` and `TypeDeclaration` (Luau). Flags on the declaration nodes mark exported and `const` Luau locals and functions.

`LastStatement` has four variants: `Return`, `Break`, `Continue` (Luau), `Error`.

### Shared types

- **`Punctuated<T>`** is a separated list, usually comma-separated: a `Vec<T>` plus a `has_trailing_separator` flag. The owning node implies the separator's spelling and position, so the type stores no separator tokens or spans. Argument lists, variable lists, field lists, and union and intersection types all use it.
- **`FunctionBody`** holds the parameters, vararg, optional return type annotation, and body block, plus an optional Luau generic list (`<T, U...>`). Every node that defines a function shares it, including Luau type functions, so it lives in `function.rs` with `Parameter`, `VarArgParam`, and `FunctionAttribute`.
- **`Parameter`** is a name with an optional Luau type annotation. It covers both function parameters and generic-for loop bindings, and the trailing `...` rides in a separately boxed `VarArgParam`.
- **Optional payload layout.** `FunctionBody.vararg` is `Option<Box<VarArgParam>>`, and `AttributedName.attribute` is `Option<Box<Attribute>>`. Boxing keeps the common attribute-free, non-vararg nodes small, at the cost of an allocation for each payload that is present, so vararg-heavy and attribute-heavy programs can use more memory than inline storage would. `FunctionSignature` and the synthesis constructors take unboxed values and box them.
- **`Field`** is a table constructor entry: keyed (`[expr] = expr`), named (`name = expr`), or positional (`expr`).

### Types

The Luau type grammar lives in `types.rs` as its own `Type` node family, held to the same 64-byte budget as `Expression` and `Statement`. `Type` covers named and generic references (`Name`, `module.Name`, `Name<args>`), table types, function types, unions and intersections, postfix optionals (`T?`), literal singletons, `typeof(expr)`, parenthesized types, explicit type packs (`(T, U)`), and variadic (`...T`) / generic (`T...`) pack elements. `Visitor` and `Transform` carry matching `visit_type` / `transform_type` methods (with `walk_type` recursion), and `Synth` builds these nodes too. Type nodes appear only in Luau sources, and the parser gates them on `LuaVersion::is_luau`.

### Traversal

`Visitor` walks the AST read-only by borrowing each node. Override a `visit_*` hook, then call the matching `self.walk_*` to recurse. Scope analysis and many linter rules are visitors.

`Transform` takes each node by value and returns a replacement. Override a `transform_*` hook, then call `self.walk_*` to apply default recursion before or after your rewrite. Every minifier pass uses this pattern.

The hooks cover blocks, statements, last statements, expressions, vars, function calls, table constructors, function bodies, and types. `Visitor` also hooks generic type lists, and `Transform` call arguments. A call or table constructor reaches its hook whether it sits in a statement, an expression, or a `f{...}` argument. The `walk_*` methods are the default recursion and are never overridden, so a hook can always fall back on them. `Transform` rewrites a call's arguments before its explicit type arguments, while `Visitor` visits them in source order.

The difference is ownership. `Visitor` borrows, so it can only read. `Transform` consumes, so it can restructure, replace, or remove nodes.

### Structural queries

`query.rs` holds small read-only structural predicates. One says whether an expression is a call or `...`, the forms that yield any number of values, and `Synth`, the minifier, and the linter share it. The rest decide where a reprinted node needs parentheses or a separator: whether an expression may stand bare in a prefix position (shared by `Synth` and the minifier), and whether a statement's or expression's first emitted token is `(` or `{`, which the codegen and formatter printers both guard against reprinting into a different parse.
