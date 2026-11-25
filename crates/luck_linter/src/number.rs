//! The number an expression evaluates to, as a double, which is how the
//! rules that check one compare it.

use std::cmp::Ordering;

use luck_ast::Expression;
use luck_token::LuaNumber;
use luck_value::{ConstantEvaluation as _, ConstantValue, ValueContext};

/// The number `expr` evaluates to, as `luck_value` evaluates it.
pub(crate) fn number_value(expr: &Expression<'_>, context: &dyn ValueContext) -> Option<f64> {
    expr.evaluate_value(context).and_then(ConstantValue::as_number).map(LuaNumber::to_f64)
}

/// Whether `expr` evaluates to a number equal to `value`, so to `0` for `-0`.
pub(crate) fn has_number_value(
    expr: &Expression<'_>,
    context: &dyn ValueContext,
    value: f64,
) -> bool {
    number_value(expr, context)
        .is_some_and(|number| number.partial_cmp(&value) == Some(Ordering::Equal))
}
