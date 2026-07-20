use luck_ast::expr::{Expression, Var};
use luck_ast::shared::Field;
use luck_token::token::TokenKind;
use luck_token::{BinOp, UnOp};

/// Extract a compile-time boolean value from a `true`/`false` literal.
pub fn extract_boolean(expr: &Expression) -> Option<bool> {
    match expr {
        Expression::True(_) => Some(true),
        Expression::False(_) => Some(false),
        Expression::Parenthesized(paren) => extract_boolean(&paren.expr),
        _ => None,
    }
}

/// Returns true if the expression is a `nil` literal (possibly parenthesized).
pub fn is_nil(expr: &Expression) -> bool {
    match expr {
        Expression::Nil(_) => true,
        Expression::Parenthesized(paren) => is_nil(&paren.expr),
        _ => false,
    }
}

/// Returns true if the expression is composed entirely of literal values (no variable reads).
/// Nested arithmetic on literals is still literal - metamethods cannot fire on primitives.
fn is_literal_expression(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::Parenthesized(paren) => is_literal_expression(&paren.expr),
        Expression::UnaryOp(unop) => is_literal_expression(&unop.operand),
        Expression::BinaryOp(binop) => {
            is_literal_expression(&binop.left) && is_literal_expression(&binop.right)
        }
        _ => false,
    }
}

/// Pure = no side effects. `allow_var_reads`: when false, variable reads are impure (value may change).
pub fn is_pure_expression(expr: &Expression, allow_var_reads: bool) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::FunctionDef(_) => true,
        Expression::Parenthesized(paren) => is_pure_expression(&paren.expr, allow_var_reads),
        Expression::UnaryOp(unop) => {
            if allow_var_reads {
                is_literal_expression(expr)
            } else {
                is_pure_expression(&unop.operand, false)
            }
        }
        Expression::BinaryOp(binop) => {
            let is_logic_op = matches!(binop.op, BinOp::And | BinOp::Or);
            if is_logic_op {
                // and/or never invoke metamethods - pure if operands are pure
                is_pure_expression(&binop.left, allow_var_reads)
                    && is_pure_expression(&binop.right, allow_var_reads)
            } else if allow_var_reads {
                is_literal_expression(expr)
            } else {
                is_pure_expression(&binop.left, false) && is_pure_expression(&binop.right, false)
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_pure_expression(key, allow_var_reads)
                    && is_pure_expression(value, allow_var_reads)
            }
            Field::Named { value, .. } => is_pure_expression(value, allow_var_reads),
            Field::Positional { value, .. } => is_pure_expression(value, allow_var_reads),
        }),
        Expression::Var(var) => {
            allow_var_reads
                && match var.as_ref() {
                    Var::Name(_) => true,
                    // Field access can raise (nil prefix) or fire __index -
                    // exactly as impure as Var::Index (hard invariant 6).
                    Var::FieldAccess(_) | Var::Index(_) => false,
                }
        }
        // VarArg (...) is pure: no side effects, value is fixed within a function body.
        // Unlike variable reads, varargs can't be reassigned after function entry.
        Expression::VarArg(_) => true,
        // Luau type casts are transparent wrappers - purity depends on the inner expression
        Expression::TypeCast(cast) => is_pure_expression(&cast.expr, allow_var_reads),
        _ => false,
    }
}

/// Returns true if the expression is guaranteed to be truthy at runtime.
/// Numbers, strings, tables, and functions are always truthy in Lua.
pub fn is_always_truthy(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_) | Expression::StringLiteral(_) | Expression::FunctionDef(_) => true,
        Expression::TableConstructor(_) => true,
        Expression::True(_) => true,
        Expression::Parenthesized(paren) => is_always_truthy(&paren.expr),
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            is_literal_expression(&unop.operand) && is_always_truthy(&unop.operand)
        }
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Len) => {
            is_literal_expression(&unop.operand)
        }
        Expression::BinaryOp(binop) => {
            let is_arithmetic = matches!(
                binop.op,
                BinOp::Add
                    | BinOp::Sub
                    | BinOp::Mul
                    | BinOp::Div
                    | BinOp::FloorDiv
                    | BinOp::Mod
                    | BinOp::Pow
                    | BinOp::Concat
                    | BinOp::BitAnd
                    | BinOp::BitOr
                    | BinOp::BitXor
                    | BinOp::Shl
                    | BinOp::Shr
            );
            is_arithmetic
                && is_literal_expression(&binop.left)
                && is_literal_expression(&binop.right)
        }
        _ => false,
    }
}

/// All Lua and Luau reserved keywords.
pub const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    "continue", // Luau
];

/// Returns true if the string is a valid Lua identifier (not a keyword).
pub fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().expect("checked non-empty above");
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    !LUA_KEYWORDS.contains(&s)
}

pub fn ident_name_string(token: &luck_token::Token) -> String {
    if let TokenKind::Identifier(ref name) = token.kind {
        name.to_string()
    } else {
        String::new()
    }
}

// Literal value semantics live in luck_token::literal - every crate
// that folds or re-emits literals must share ONE decode/encode.
pub use luck_token::literal::{LuaNumber, decode_string_literal, encode_string_literal};

/// Extract a number with subtype fidelity. With `int_subtype` false
/// (5.1/5.2/Luau) every number is a Float, mirroring the single f64 type.
pub fn extract_lua_number(expr: &Expression, int_subtype: bool) -> Option<LuaNumber> {
    match expr {
        Expression::Number(literal) => {
            luck_token::literal::parse_lua_number(&literal.text, int_subtype)
        }
        Expression::UnaryOp(unop) if matches!(unop.op, UnOp::Neg) => {
            match extract_lua_number(&unop.operand, int_subtype)? {
                LuaNumber::Int(value) => Some(LuaNumber::Int(value.wrapping_neg())),
                LuaNumber::Float(value) => Some(LuaNumber::Float(-value)),
            }
        }
        Expression::Parenthesized(paren) => extract_lua_number(&paren.expr, int_subtype),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_identifier() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_bar"));
        assert!(is_valid_identifier("x1"));
        assert!(is_valid_identifier("_"));
        assert!(is_valid_identifier("camelCase"));

        assert!(!is_valid_identifier("if"));
        assert!(!is_valid_identifier("end"));
        assert!(!is_valid_identifier("and"));
        assert!(!is_valid_identifier("or"));

        assert!(!is_valid_identifier("a-b"));
        assert!(!is_valid_identifier("123abc"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("foo bar"));
    }
}
