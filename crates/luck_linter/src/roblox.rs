//! Helpers for the Roblox rules that check `<Global>.new(...)` calls with
//! literal numeric arguments.

use core::ops::Neg;

use luck_ast::{Expression, FunctionArgs, FunctionCall, Literal, Punctuated, Var};
use luck_semantic::SemanticAnalysis;
use luck_token::{LuaNumber, NumberSubtypes, UnOp, parse_lua_number};

/// The call and its arguments when `expr` is a parenthesized
/// `<global>.new(...)` call under the Roblox stdlib, and `<global>` isn't
/// shadowed by a local.
pub(crate) fn global_new_call<'expr>(
    expr: &'expr Expression,
    global: &str,
    semantic: &SemanticAnalysis,
) -> Option<(&'expr FunctionCall, &'expr Punctuated<Expression>)> {
    if !semantic.environment.is_roblox() {
        return None;
    }
    let Expression::FunctionCall(call) = expr else {
        return None;
    };
    if call.method.is_some() {
        return None;
    }
    let Expression::Var(Var::FieldAccess(field)) = &call.callee else {
        return None;
    };
    if field.name.identifier() != Some("new") {
        return None;
    }
    let Expression::Var(Var::Name(token)) = &field.prefix else {
        return None;
    };
    if token.identifier() != Some(global) || semantic.resolves_to_local(global, token.span) {
        return None;
    }
    let FunctionArgs::Parenthesized { args, .. } = &call.args else {
        return None;
    };
    Some((call, args))
}

/// Numeric value of a literal argument, allowing a single unary minus.
/// Non-literal expressions (identifiers, calls) yield `None`.
pub(crate) fn literal_arg_value(expr: &Expression) -> Option<f64> {
    if let Expression::Number(literal) = expr {
        return number_value(literal);
    }
    let Expression::UnaryOp(unary) = expr else {
        return None;
    };
    let Expression::Number(literal) = &unary.operand else {
        return None;
    };
    if unary.op != UnOp::Neg {
        return None;
    }
    number_value(literal).map(Neg::neg)
}

fn number_value(literal: &Literal) -> Option<f64> {
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat)? {
        #[expect(
            clippy::as_conversions,
            clippy::cast_precision_loss,
            reason = "Luau numbers are doubles, so this rounding is the conversion wanted"
        )]
        LuaNumber::Int(int_value) => Some(int_value as f64),
        LuaNumber::Float(float_value) => Some(float_value),
    }
}
