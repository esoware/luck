# luck_value

Lua's value semantics over the syntax tree, which `luck_minifier` and `luck_linter` share, as `oxc_ecmascript` holds ECMAScript's for oxc.

## Overview

One analysis of what evaluating an expression does answers every question, so the answers agree. It assumes that every metamethod can run wherever Lua allows one and do anything, and that the environment can have a metatable. Reading a global indexes the environment, so it may run `__index`, and reading a local or an upvalue runs nothing. An operator is pure only on values whose types rule a metamethod and an error out, such as two numbers for `+`, and an index or a call never is.

## Usage

- `ToBoolean::to_boolean`: whether an expression's value is truthy, which every value but `nil` and `false` is. `{f()}` is truthy, and so are `x or true` and a string.
- `ConstantEvaluation::evaluate_value`: the `ConstantValue` an expression evaluates to, when evaluating it can't raise, run a metamethod, or read a global. It's in the target's exact arithmetic: integer operators wrap, float `%` is each version's, an integer `%` or `//` by zero raises, and a result the target computes through the C library, a power that isn't an exact integer or a string comparison, has no value. `get_side_free_boolean_value` gives the truthiness of an expression a rewrite can drop.
- `MayHaveSideEffects::may_have_side_effects`: whether evaluating an expression may raise, run a metamethod, or read a global.
- `IsLiteralValue::is_literal_value`: whether an expression evaluates to the same value wherever and whenever it runs, so a rewrite may move it: it has no effect, reads no variable and no `...`, and builds no table or closure.
- `ConstantValue::binary`: a binary operator on two constant values, for a rewrite that folds part of a run.
- `ValueContext`: what the analysis has to know of an expression's place, its dialect and which names are globals. A `LuaVersion` is one that takes every name for a global.

```rust
use luck_allocator::Allocator;
use luck_ast::Statement;
use luck_token::{LuaNumber, LuaVersion};
use luck_value::{ConstantEvaluation as _, ConstantValue, MayHaveSideEffects as _, ToBoolean as _};

let allocator = Allocator::new();
let parsed = luck_parser::parse(&allocator, b"return 7 // 2, x or true", LuaVersion::Lua54);
let Some(Statement::Return(stmt)) = parsed.chunk.block.stmts.first() else {
    unreachable!("the chunk is one return");
};
let [quotient, guard] = &*stmt.exprs else { unreachable!("it returns two values") };
let version = LuaVersion::Lua54;
assert_eq!(quotient.evaluate_value(&version), Some(ConstantValue::Number(LuaNumber::Int(3))));
assert_eq!(guard.to_boolean(&version), Some(true));
assert!(guard.may_have_side_effects(&version), "`x` may be a global");
```
