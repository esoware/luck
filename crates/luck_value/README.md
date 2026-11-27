# luck_value

Constant evaluation, truthiness, and side-effect analysis for Luau expressions.

Four traits answer questions about an expression without running it.
`ConstantEvaluation` works out its `ConstantValue`, `ToBoolean` whether it's
truthy, `MayHaveSideEffects` whether evaluating it could raise, run a
metamethod, or read a global, and `IsLiteralValue` whether it has the same
value wherever and whenever it runs.

The answers assume any metamethod can run wherever Luau allows one. Reading a
global counts as a side effect, because it indexes the environment, which may
have an `__index`. An operator is only pure when its operand types rule out
metamethods and errors, such as `+` on two numbers.

Constant evaluation matches Luau's arithmetic exactly. Anything that depends on
the C library, such as an inexact power or a string comparison, is left
unevaluated.

Each trait takes a `ValueContext`, which says which names are globals.
`NoBindings` treats every name as a global, the safe choice when there's no
scope information.

## Usage

This example evaluates `7 // 2`, and shows that `x or true` is truthy but may
still have side effects:

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
