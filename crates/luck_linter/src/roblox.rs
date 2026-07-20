//! Helpers shared by the Roblox-specific rules that inspect
//! `<Global>.new(...)` constructor calls with literal numeric arguments
//! (`roblox_incorrect_color3_new_bounds`, `roblox_suspicious_udim2_new`,
//! `roblox_manual_fromscale_or_fromoffset`).

use luck_ast::expr::{Expression, FunctionCall, Var};
use luck_token::TokenKind;
use luck_token::literal::{LuaNumber, NumberSubtypes, parse_lua_number};

use crate::rule::LintContext;

/// Whether `call` is `<global>.new(...)` where `<global>` is the real
/// Roblox global (not a shadowing local) and there is no method receiver.
pub(crate) fn is_global_new_call(call: &FunctionCall, global: &str, ctx: &LintContext) -> bool {
    if call.method.is_some() {
        return false;
    }
    let Expression::Var(Var::FieldAccess(field)) = &call.callee else {
        return false;
    };
    if !matches!(&field.name.kind, TokenKind::Identifier(name) if name == "new") {
        return false;
    }
    let Expression::Var(Var::Name(token)) = &field.prefix else {
        return false;
    };
    matches!(&token.kind, TokenKind::Identifier(name) if name == global)
        && !ctx.semantic.resolves_to_local(global, token.span)
}

/// Numeric value of a literal argument, allowing a single unary minus.
/// Non-literal expressions (identifiers, calls) yield `None`.
pub(crate) fn literal_arg_value(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Number(literal) => number_value(literal),
        Expression::UnaryOp(unary) if unary.op == luck_token::UnOp::Neg => {
            if let Expression::Number(literal) = &unary.operand {
                number_value(literal).map(|value| -value)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn number_value(literal: &luck_ast::expr::Literal) -> Option<f64> {
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat)? {
        LuaNumber::Int(int_value) => Some(int_value as f64),
        LuaNumber::Float(float_value) => Some(float_value),
    }
}
