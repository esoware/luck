# luck_value

Works out what Luau expressions evaluate to, whether they're truthy, and whether evaluating them has side effects.

## Overview

The minifier and the linter both use it, the way oxc's tools use `oxc_ecmascript`.

It assumes any metamethod can run wherever Luau allows one. Reading a global indexes the environment, so it can call `__index`. Reading a local can't.

An operator only counts as pure when its operand types rule out metamethods and errors, like `+` on two numbers.

Constant evaluation matches Luau's arithmetic exactly. Anything that depends on the C library, such as an inexact power or a string comparison, isn't folded.

Each trait takes a `ValueContext` that says which names are globals. `NoBindings` treats every name as a global.

## Usage

```rust
use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_value::{
    ConstantEvaluation as _, ConstantValue, MayHaveSideEffects as _, NoBindings, ToBoolean as _,
};

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"return 7 // 2, x or true");
let Some(Statement::Return(stmt)) = parsed.chunk.stmts.first() else {
    unreachable!("the chunk is one return");
};
let [quotient, guard] = &*stmt.exprs else { unreachable!("it returns two values") };
assert_eq!(quotient.evaluate_value(&NoBindings), Some(ConstantValue::Number(3.0)));
assert_eq!(guard.to_boolean(&NoBindings), Some(true));
assert!(guard.may_have_side_effects(&NoBindings), "`x` may be a global");
```

## API

- `ToBoolean`, `ConstantEvaluation`, `MayHaveSideEffects`, and `IsLiteralValue` are traits on expressions.
- `ConstantValue` is a value known without running the program.
- `ValueContext` tells locals from globals.
- `NoBindings` is a context where every name is a global.
