use core::ops::ControlFlow::Continue;

use luck_ast::{
    BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, FunctionCall, FunctionDef,
    IfExpression, InterpolatedString, TableConstructor, TypeCast, TypeInstantiation, UnaryOp, Var,
    is_multi_value,
};
use luck_token::LuaVersion;

use super::function::{function_attributes_eq, function_body_eq};
use super::types::type_eq;
use super::{
    Check, at, at_index, differ, presence_eq, punctuated_eq, require, string_raw_eq, token_text_eq,
};

// Each kind is compared in a function of its own, so a level of nesting
// holds only that comparison's temporaries on the stack.
pub(super) fn expr_eq(left: &Expression, right: &Expression, version: LuaVersion) -> Check {
    match (left, right) {
        (Expression::Nil(_), Expression::Nil(_))
        | (Expression::False(_), Expression::False(_))
        | (Expression::True(_), Expression::True(_))
        | (Expression::VarArg(_), Expression::VarArg(_))
        | (Expression::Error(_), Expression::Error(_)) => Continue(()),
        (Expression::InterpolatedString(lhs), Expression::InterpolatedString(rhs)) => {
            at("InterpolatedString", interpolated_string_eq(lhs, rhs, version))
        }
        (Expression::StringLiteral(lhs), Expression::StringLiteral(rhs)) => {
            // The formatter may swap quote style, so compare decoded contents.
            require(string_raw_eq(&lhs.text, &rhs.text, version), "literal contents differ")
        }
        (Expression::Number(lhs), Expression::Number(rhs)) => {
            require(number_raw_eq(&lhs.text, &rhs.text), "number literals differ")
        }
        (Expression::Integer(lhs), Expression::Integer(rhs)) => {
            require(number_raw_eq(&lhs.text, &rhs.text), "integer literals differ")
        }
        (Expression::Var(lhs), Expression::Var(rhs)) => var_eq(lhs, rhs, version),
        (Expression::BinaryOp(lhs), Expression::BinaryOp(rhs)) => binary_op_eq(lhs, rhs, version),
        (Expression::UnaryOp(lhs), Expression::UnaryOp(rhs)) => unary_op_eq(lhs, rhs, version),
        (Expression::Parenthesized(lhs), Expression::Parenthesized(rhs)) => {
            at("Parenthesized/expr", expr_eq(&lhs.expr, &rhs.expr, version))
        }
        // Redundant parens may come and go, so `(x)` matches `x` however deep
        // the nesting. Parens around a call or `...` truncate it to one value
        // and have to match.
        (Expression::Parenthesized(lhs), other) => {
            redundant_parens_eq(&lhs.expr, &lhs.expr, other, version)
        }
        (other, Expression::Parenthesized(rhs)) => {
            redundant_parens_eq(&rhs.expr, other, &rhs.expr, version)
        }
        (Expression::FunctionCall(lhs), Expression::FunctionCall(rhs)) => {
            at("FunctionCall", function_call_eq(lhs, rhs, version))
        }
        (Expression::FunctionDef(lhs), Expression::FunctionDef(rhs)) => {
            function_def_eq(lhs, rhs, version)
        }
        (Expression::TableConstructor(lhs), Expression::TableConstructor(rhs)) => {
            at("TableConstructor", table_eq(lhs, rhs, version))
        }
        (Expression::IfExpression(lhs), Expression::IfExpression(rhs)) => {
            if_expr_eq(lhs, rhs, version)
        }
        (Expression::TypeCast(lhs), Expression::TypeCast(rhs)) => type_cast_eq(lhs, rhs, version),
        (Expression::TypeInstantiation(lhs), Expression::TypeInstantiation(rhs)) => {
            type_instantiation_eq(lhs, rhs, version)
        }
        (
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::VarArg(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_)
            | Expression::StringLiteral(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::Var(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::FunctionCall(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_),
            _,
        ) => expr_kinds_differ(left, right),
    }
}

fn binary_op_eq(left: &BinaryOp, right: &BinaryOp, version: LuaVersion) -> Check {
    require(left.op == right.op, "binary op differs")?;
    at("left", expr_eq(&left.lhs, &right.lhs, version))?;
    at("right", expr_eq(&left.rhs, &right.rhs, version))
}

fn unary_op_eq(left: &UnaryOp, right: &UnaryOp, version: LuaVersion) -> Check {
    require(left.op == right.op, "unary op differs")?;
    at("operand", expr_eq(&left.operand, &right.operand, version))
}

/// Compares the two sides once one of them has shed the parentheses around
/// `inner`.
fn redundant_parens_eq(
    inner: &Expression,
    left: &Expression,
    right: &Expression,
    version: LuaVersion,
) -> Check {
    require(!is_multi_value(inner), "value-truncating parens differ")?;
    expr_eq(left, right, version)
}

fn type_cast_eq(left: &TypeCast, right: &TypeCast, version: LuaVersion) -> Check {
    at("TypeCast/expr", expr_eq(&left.expr, &right.expr, version))?;
    at("TypeCast/type_annotation", type_eq(&left.type_annotation, &right.type_annotation, version))
}

#[cold]
#[inline(never)]
fn expr_kinds_differ(left: &Expression, right: &Expression) -> Check {
    differ(format!("expression kind differs: `{}` vs `{}`", expr_kind(left), expr_kind(right)))
}

fn interpolated_string_eq(
    left: &InterpolatedString,
    right: &InterpolatedString,
    version: LuaVersion,
) -> Check {
    require(left.segments.len() == right.segments.len(), "interpolated segment count differs")?;
    for (i, (lhs, rhs)) in left.segments.iter().zip(&right.segments).enumerate() {
        at_index("segments", i, token_text_eq(&lhs.literal, &rhs.literal))?;
        presence_eq(
            lhs.expr.as_ref(),
            rhs.expr.as_ref(),
            "interpolated expression presence differs",
            |left_expr, right_expr| {
                at_index("segments", i, at("expr", expr_eq(left_expr, right_expr, version)))
            },
        )?;
    }
    Continue(())
}

fn function_def_eq(left: &FunctionDef, right: &FunctionDef, version: LuaVersion) -> Check {
    at("FunctionDef", function_attributes_eq(&left.attributes, &right.attributes, version))?;
    at("FunctionDef/body", function_body_eq(&left.body, &right.body, version))
}

fn type_instantiation_eq(
    left: &TypeInstantiation,
    right: &TypeInstantiation,
    version: LuaVersion,
) -> Check {
    at("TypeInstantiation/expr", expr_eq(&left.expr, &right.expr, version))?;
    at(
        "TypeInstantiation/type_args",
        punctuated_eq(&left.type_args.args, &right.type_args.args, version, type_eq),
    )
}

fn if_expr_eq(left: &IfExpression, right: &IfExpression, version: LuaVersion) -> Check {
    at("IfExpression/condition", expr_eq(&left.condition, &right.condition, version))?;
    at("IfExpression/then_expr", expr_eq(&left.then_expr, &right.then_expr, version))?;
    require(
        left.elseif_clauses.len() == right.elseif_clauses.len(),
        "if-expression `elseif` count differs",
    )?;
    let clause_eq = |lhs: &ElseIfExprClause, rhs: &ElseIfExprClause| {
        at("condition", expr_eq(&lhs.condition, &rhs.condition, version))?;
        at("expr", expr_eq(&lhs.expr, &rhs.expr, version))
    };
    let clauses = left.elseif_clauses.iter().zip(&right.elseif_clauses);
    for (i, (lhs, rhs)) in clauses.enumerate() {
        at_index("IfExpression/elseif_clauses", i, clause_eq(lhs, rhs))?;
    }
    at("IfExpression/else_expr", expr_eq(&left.else_expr, &right.else_expr, version))
}

pub(super) fn var_eq(left: &Var, right: &Var, version: LuaVersion) -> Check {
    match (left, right) {
        (Var::Name(lhs), Var::Name(rhs)) => token_text_eq(lhs, rhs),
        (Var::FieldAccess(lhs), Var::FieldAccess(rhs)) => {
            at("prefix", expr_eq(&lhs.prefix, &rhs.prefix, version))?;
            at("name", token_text_eq(&lhs.name, &rhs.name))
        }
        (Var::Index(lhs), Var::Index(rhs)) => {
            at("prefix", expr_eq(&lhs.prefix, &rhs.prefix, version))?;
            at("index", expr_eq(&lhs.index, &rhs.index, version))
        }
        (Var::Name(_) | Var::FieldAccess(_) | Var::Index(_), _) => differ("var kind differs"),
    }
}

pub(super) fn function_call_eq(
    left: &FunctionCall,
    right: &FunctionCall,
    version: LuaVersion,
) -> Check {
    at("callee", expr_eq(&left.callee, &right.callee, version))?;
    presence_eq(
        left.method.as_ref(),
        right.method.as_ref(),
        "method presence differs",
        |lhs, rhs| at("method", token_text_eq(lhs, rhs)),
    )?;
    presence_eq(
        left.explicit_type_args.as_deref(),
        right.explicit_type_args.as_deref(),
        "explicit type argument presence differs",
        |lhs, rhs| at("explicit_type_args", punctuated_eq(&lhs.args, &rhs.args, version, type_eq)),
    )?;
    at("args", args_eq(&left.args, &right.args, version))
}

fn args_eq(left: &FunctionArgs, right: &FunctionArgs, version: LuaVersion) -> Check {
    match (left, right) {
        (
            FunctionArgs::Parenthesized { args: lhs, .. },
            FunctionArgs::Parenthesized { args: rhs, .. },
        ) => punctuated_eq(lhs, rhs, version, expr_eq),
        (FunctionArgs::TableConstructor(lhs), FunctionArgs::TableConstructor(rhs)) => {
            table_eq(lhs, rhs, version)
        }
        (FunctionArgs::StringLiteral(lhs), FunctionArgs::StringLiteral(rhs)) => {
            require(string_raw_eq(&lhs.text, &rhs.text, version), "call string arg differs")
        }
        // The formatter may add or drop the parens in `f("x")` and `f({1})`.
        (FunctionArgs::StringLiteral(literal), FunctionArgs::Parenthesized { args, .. })
        | (FunctionArgs::Parenthesized { args, .. }, FunctionArgs::StringLiteral(literal)) => {
            if args.len() == 1
                && let Some(Expression::StringLiteral(other)) = args.first()
                && string_raw_eq(&literal.text, &other.text, version)
            {
                return Continue(());
            }
            differ("call arg shape differs")
        }
        (FunctionArgs::TableConstructor(table), FunctionArgs::Parenthesized { args, .. })
        | (FunctionArgs::Parenthesized { args, .. }, FunctionArgs::TableConstructor(table)) => {
            if args.len() == 1
                && let Some(Expression::TableConstructor(other)) = args.first()
            {
                return table_eq(table, other, version);
            }
            differ("call arg shape differs")
        }
        (FunctionArgs::StringLiteral(_), FunctionArgs::TableConstructor(_))
        | (FunctionArgs::TableConstructor(_), FunctionArgs::StringLiteral(_)) => {
            differ("call arg shape differs")
        }
    }
}

fn table_eq(left: &TableConstructor, right: &TableConstructor, version: LuaVersion) -> Check {
    if left.fields.len() != right.fields.len() {
        return differ(format!(
            "table field count differs: {} vs {}",
            left.fields.len(),
            right.fields.len()
        ));
    }
    for (i, (lhs, rhs)) in left.fields.iter().zip(right.fields.iter()).enumerate() {
        at_index("fields", i, field_eq(lhs, rhs, version))?;
    }
    Continue(())
}

fn field_eq(left: &Field, right: &Field, version: LuaVersion) -> Check {
    match (left, right) {
        (
            Field::Named { name: left_name, value: left_value, .. },
            Field::Named { name: right_name, value: right_value, .. },
        ) => {
            at("name", token_text_eq(left_name, right_name))?;
            at("value", expr_eq(left_value, right_value, version))
        }
        (
            Field::Bracketed { key: left_key, value: left_value, .. },
            Field::Bracketed { key: right_key, value: right_value, .. },
        ) => {
            at("key", expr_eq(left_key, right_key, version))?;
            at("value", expr_eq(left_value, right_value, version))
        }
        (
            Field::Positional { value: left_value, .. },
            Field::Positional { value: right_value, .. },
        ) => expr_eq(left_value, right_value, version),
        (Field::Named { .. } | Field::Bracketed { .. } | Field::Positional { .. }, _) => {
            differ("field kind differs")
        }
    }
}

/// Compare two raw number texts, tolerating only the formatter's own
/// rewrites.
///
/// Those are lowercasing the base prefix and exponent marker, changing hex
/// digit case, and adding a zero before a bare leading dot (`.5` -> `0.5`).
///
/// A case-insensitive comparison after undoing the leading zero allows just
/// those. `0x10` and `16` still differ, since the formatter never changes
/// base.
fn number_raw_eq(left_raw: &str, right_raw: &str) -> bool {
    without_bare_zero(left_raw).eq_ignore_ascii_case(without_bare_zero(right_raw))
}

fn without_bare_zero(raw: &str) -> &str {
    raw.strip_prefix('0').filter(|rest| rest.starts_with('.')).unwrap_or(raw)
}

const fn expr_kind(expr: &Expression) -> &'static str {
    match expr {
        Expression::Nil(_) => "Nil",
        Expression::False(_) => "False",
        Expression::True(_) => "True",
        Expression::Number(_) => "Number",
        // Luau
        Expression::Integer(_) => "Integer",
        Expression::StringLiteral(_) => "StringLiteral",
        Expression::VarArg(_) => "VarArg",
        Expression::FunctionDef(_) => "FunctionDef",
        Expression::Var(_) => "Var",
        Expression::FunctionCall(_) => "FunctionCall",
        Expression::Parenthesized(_) => "Parenthesized",
        Expression::TableConstructor(_) => "TableConstructor",
        Expression::BinaryOp(_) => "BinaryOp",
        Expression::UnaryOp(_) => "UnaryOp",
        // Luau
        Expression::IfExpression(_) => "IfExpression",
        // Luau
        Expression::InterpolatedString(_) => "InterpolatedString",
        // Luau
        Expression::TypeCast(_) => "TypeCast",
        // Luau
        Expression::TypeInstantiation(_) => "TypeInstantiation",
        Expression::Error(_) => "Error",
    }
}
