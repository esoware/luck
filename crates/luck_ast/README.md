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
- **Programmatic synthesis.** `Synth` (`synth.rs`, its constructors split by node family under `synth/`) builds AST nodes without any source text, handing out fresh, increasing point spans. `Synth::share` gives another synthesizer on the same counter, so several of them, even on different threads, can feed one tree without span collisions. `Synth::starting_at` starts the counter at a given offset, so nodes spliced into a parsed AST can sit above its byte offsets. Constructors take `&self`, so calls nest. `binary_op`, `unary_op`, and `type_cast` parenthesize operands by precedence, prefix positions wrap expressions that can't stand there, and the string and number constructors handle escaping and values with no literal form (negatives, infinities, NaN, `i64::MIN`, non-UTF-8 byte strings). Every constructor that takes a name or literal text asserts it's one, so a tree built only with `Synth` holds no leaf a printer refuses. It's meant for tools that emit an AST directly, and `SyntheticComment` carries comments, leading or trailing like a lexed `Comment`, for the formatter to attach to nodes. The node types themselves have no builder methods. Their `span()` accessors live in `span.rs`, and the `Punctuated<T>` helpers sit beside the type in `punctuated.rs`.

## Architecture

### Core types

A `Block` is the fundamental unit: a sequence of `Statement`s followed by an optional `LastStatement`. Every function body, loop body, and `do...end` contains one.

`Expression` covers literals (`Nil`, `False`, `True`, `Number`, Luau `Integer`, `StringLiteral`, `VarArg`), `Name`, compound forms (`Suffixed`, `BinaryOp`, `UnaryOp`, `Parenthesized`, `TableConstructor`, `FunctionDef`), and Luau extensions (`IfExpression`, `InterpolatedString`, `TypeCast`).

A run is one node however long it gets. A `SuffixedExpression` is a prefix and the suffixes after it, each a field, an index, a call or method call, or a Luau type instantiation, so `a.b[c]:d()` is one node. A `BinaryOp` is a first operand and the links after it, each an operator and its operand, all at one precedence level, so `a + b - c` is one node. A left-associative level groups from the left, and the right-associative `..` and `^` group from the right. Both keep their tail in a `OneOrMore`, which holds a lone item inline and two or more in one slice of exactly their length. `ChainHead` borrows a chain's prefix and its first few suffixes as one expression, and `Call` borrows one call in a chain with that head as its callee.

`Statement` covers the imperative side: `Assignment`, `FunctionCall`, `DoBlock`, `WhileLoop`, `RepeatLoop`, `IfStatement`, `NumericFor`, `GenericFor`, `FunctionDecl`, `LocalFunction`, `LocalAssignment`, plus the version-gated statements `Goto` / `Label` and mid-block `Break` (5.2+), attribute-bearing `LocalAssignment` (5.4+), `GlobalDeclaration` / `GlobalFunction` / `GlobalStar` (5.5), `CompoundAssignment` and `TypeDeclaration` (Luau). Flags on the declaration nodes mark exported and `const` Luau locals and functions.

A `FunctionCall` statement holds a chain that ends in a call, and an assignment target is a `Var`, a name or a chain that ends in a field or an index.

`LastStatement` has four variants: `Return`, `Break`, `Continue` (Luau), `Error`.

### Shared types

- **`Punctuated<T>`** is a separated list, usually comma-separated: a `Vec<T>` plus a `has_trailing_separator` flag. The owning node implies the separator's spelling and position, so the type stores no separator tokens or spans. Argument lists, variable lists, field lists, and union and intersection types all use it.
- **`FunctionBody`** holds the parameters, vararg, optional return type annotation, and body block, plus an optional Luau generic list (`<T, U...>`). Every node that defines a function shares it, including Luau type functions, so it lives in `function.rs` with `Parameter`, `VarArgParam`, and `FunctionAttribute`.
- **`Parameter`** is a name with an optional Luau type annotation. It covers both function parameters and generic-for loop bindings, and the trailing `...` rides in a separately boxed `VarArgParam`.
- **Optional payload layout.** `FunctionBody.vararg` is `Option<Box<VarArgParam>>`, and `AttributedName.attribute` is `Option<Box<Attribute>>`. Boxing keeps the common attribute-free, non-vararg nodes small, at the cost of an allocation for each payload that is present, so vararg-heavy and attribute-heavy programs can use more memory than inline storage would. `FunctionSignature` and the synthesis constructors take unboxed values and box them.
- **`Field`** is a table constructor entry: keyed (`[expr] = expr`), named (`name = expr`), or positional (`expr`).

### Types

The Luau type grammar lives in `types.rs` as its own `Type` node family, held to the same 64-byte budget as `Expression` and `Statement`. `Type` covers named and generic references (`Name`, `module.Name`, `Name<args>`), table types, function types, unions and intersections, postfix optionals (`T?`), literal singletons, `typeof(expr)`, parenthesized types, explicit type packs (`(T, U)`), and variadic (`...T`) / generic (`T...`) pack elements. `Visitor` and `Transform` carry matching `visit_type` / `transform_type` methods (with `walk_type` recursion), and `Synth` builds these nodes too. Type nodes appear only in Luau sources, and the parser gates them on `LuaVersion::has_type_syntax`.

### Traversal

`Visitor` walks the AST read-only by borrowing each node. Override a `visit_*` hook, then call the matching `self.walk_*` to recurse. Scope analysis and many linter rules are visitors.

`Transform` takes each node by value and returns a replacement. Override a `transform_*` hook, then call `self.walk_*` to apply default recursion before or after your rewrite. Every minifier pass uses this pattern.

The hooks cover blocks, statements, last statements, expressions, assignment targets, suffixed expressions, table constructors, function bodies, and types. `Visitor` also hooks each call in a chain and generic type lists, and `Transform` each call's arguments. A chain reaches its hook whether it's an expression, a call statement, or an assignment target, and a table constructor whether it's an expression or a `f{...}` argument. The `walk_*` methods are the default recursion and are never overridden, so a hook can always fall back on them. Both traits reach a chain's suffixes in source order.

The difference is ownership. `Visitor` borrows, so it can only read. `Transform` consumes, so it can restructure, replace, or remove nodes.

### Nesting depth

Every traversal recurses once per level of nesting, drop and clone included, so the depth of a tree is what bounds their stack. `depth.rs` defines the level and the cap, `MAX_NESTING_DEPTH` (256). A level is a block, a function body, a table constructor, a run of suffixes, a run of binary operators, a unary operator, a parenthesized expression, a Luau if-expression, interpolated string, or cast, or a Luau type that holds other types or an expression. Names, literals, and statements add none, and a run is one level however long it gets, so the depth of a parsed tree is the source's real nesting.

The walkers call `descend` on entering each level and `ascend` on leaving it, on both traits, so a pass tracks its depth by overriding the pair. `Visitor::descend` returns whether to walk what the level holds. `block_nesting_depth` and `expr_nesting_depth` measure a tree that way and stop at one past the cap, so they are safe on a tree of any depth. `luck_parser` builds no tree deeper than the cap, and `luck_codegen::compact` and `luck_formatter::format_block` return an error for one that is. Nothing checks a `Synth` tree as it grows, and dropping or cloning one nested far past the cap can overflow the stack.

A rewrite can leave a run as a bare operand of a run at its own level, such as `a or (b or c)` built with no `Parenthesized` node, or a chain as another chain's prefix. Each prints as one run and reads back as one, so a tree is never shallower than what it prints. `BinaryOp::splice_operands` and `SuffixedExpression::splice_prefix` merge them back into one node, and `Synth` merges as it builds.

### Leaf spellings

The node fields are public, so a tree built in code can hold a leaf no source spells where it sits: a keyword or `Eof` token, or the parser's empty placeholder, where a name belongs, number or string text that isn't one token of its kind, `typeof` where a Luau type name starts, a table type field that a bare `read` or `write` opens, an interpolated string whose segments aren't shaped as the lexer reads them, or an error node. `spelling.rs` gives each leaf's text, or the `SourceError` for one with none, through `name_spelling`, `type_name_spelling`, `number_spelling`, `integer_spelling`, `string_spelling`, `singleton_spelling`, `access_spelling`, `check_type_field`, `check_interpolated_string`, and `error_node`. `luck_codegen::compact` and `luck_formatter::format_block` take every leaf through them, so they refuse the same trees with the same error. Neither printer knows the target dialect, so a leaf passes when it reads back in some dialect, as the name `goto` does.

### Structural queries

`query.rs` holds small read-only structural predicates. One says whether an expression is a call or `...`, the forms that yield any number of values, and `Synth`, the minifier, and the linter share it. The rest decide where a reprinted node needs parentheses or a separator: whether an expression may stand bare in a prefix position (shared by `Synth` and the minifier), and whether a statement's or expression's first emitted token is `(` or `{`, which the codegen and formatter printers both guard against reprinting into a different parse.
