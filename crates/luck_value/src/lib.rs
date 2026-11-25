//! # `luck_value`
//!
//! Luau's value semantics over the syntax tree, which `luck_minifier` and
//! `luck_linter` share, as `oxc_ecmascript` holds ECMAScript's for oxc.
//!
//! - [`ToBoolean`] tells whether an expression's value is truthy, which
//!   every value but `nil` and `false` is.
//! - [`ConstantEvaluation`] gives the [`ConstantValue`] an expression
//!   evaluates to, in Luau's exact arithmetic: a result Luau computes
//!   through the C library, a power that isn't an exact integer or a string
//!   comparison, has no value.
//! - [`MayHaveSideEffects`] tells whether evaluating an expression may raise,
//!   run a metamethod, or read a global.
//! - [`IsLiteralValue`] tells whether an expression evaluates to the same
//!   value wherever and whenever it runs, so a rewrite may move it.
//!
//! The answers assume that every metamethod can run wherever Luau allows
//! one and do anything, and that the environment can have a metatable.
//! Reading a global indexes the environment, so it may run `__index`, and
//! reading a local or an upvalue runs nothing. An operator is pure only on
//! values whose types rule a metamethod and an error out, such as two
//! numbers for `+`, and an index or a call never is.
//!
//! Each takes a [`ValueContext`], which says which names are globals.
//! [`NoBindings`] knows no bindings, so it takes every name for a global.
//!
//! # Usage
//!
//! ```
//! use luck_allocator::Allocator;
//! use luck_ast::Statement;
//! use luck_value::{
//!     ConstantEvaluation as _, ConstantValue, MayHaveSideEffects as _, NoBindings, ToBoolean as _,
//! };
//!
//! let allocator = Allocator::new();
//! let parsed = luck_parser::parse(&allocator, b"return 7 // 2, x or true");
//! let Some(Statement::Return(stmt)) = parsed.chunk.block.stmts.first() else {
//!     unreachable!("the chunk is one return");
//! };
//! let [quotient, guard] = &*stmt.exprs else { unreachable!("it returns two values") };
//! assert_eq!(quotient.evaluate_value(&NoBindings), Some(ConstantValue::Number(3.0)));
//! assert_eq!(guard.to_boolean(&NoBindings), Some(true));
//! assert!(guard.may_have_side_effects(&NoBindings), "`x` may be a global");
//! ```

mod context;
mod expression;
mod value;

pub use context::{NoBindings, ValueContext};
pub use expression::{ConstantEvaluation, IsLiteralValue, MayHaveSideEffects, ToBoolean};
pub use value::ConstantValue;
