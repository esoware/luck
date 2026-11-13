use core::iter;

use luck_ast::{
    BinaryOp, Expression, Field, Literal, MAX_NESTING_DEPTH, ParenthesizedExpression, Suffix,
    SuffixedExpression, expr_nesting_depth, is_multi_value, unwrap_parentheses,
};
use luck_token::{
    Associativity, BinOp, LuaNumber, NumberSubtypes, OperandSide, Span, UnOp, parse_lua_number,
};

pub(crate) fn extract_boolean(expr: &Expression) -> Option<bool> {
    match unwrap_parentheses(expr) {
        Expression::True(_) => Some(true),
        Expression::False(_) => Some(false),
        Expression::Parenthesized(_)
        | Expression::Nil(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

pub(crate) fn is_nil(expr: &Expression) -> bool {
    match unwrap_parentheses(expr) {
        Expression::Nil(_) => true,
        Expression::Parenthesized(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// What the operators that can raise care about in a literal operand.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    /// A number with no known exact integer representation.
    Float,
    /// A number the bitwise operators accept.
    Integer,
    String,
    /// `nil`, a boolean, a Luau integer literal, or the result of `not`, an
    /// equality, or a comparison: no operator but `not`, `==`, `~=`, `and`, or
    /// `or` accepts one.
    Other,
}

impl LiteralKind {
    const fn is_number(self) -> bool {
        matches!(self, Self::Float | Self::Integer)
    }

    const fn is_concatenable(self) -> bool {
        matches!(self, Self::Float | Self::Integer | Self::String)
    }
}

/// The kind of a literal expression whose evaluation can't raise, or `None`
/// for anything else. Operators are checked against the operand kinds, since
/// `nil + 1` and `1 < "a"` raise with no variable in sight.
fn literal_kind(expr: &Expression) -> Option<LiteralKind> {
    match expr {
        Expression::Number(literal) => Some(number_literal_kind(literal)),
        Expression::StringLiteral(_) => Some(LiteralKind::String),
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Integer(_) => Some(LiteralKind::Other),
        Expression::Parenthesized(paren) => literal_kind(unwrap_parentheses(&paren.expr)),
        Expression::UnaryOp(unop) => {
            let operand = literal_kind(&unop.operand)?;
            unary_literal_kind(unop.op, operand)
        }
        Expression::BinaryOp(binary_op) => run_literal_kind(binary_op),
        // Luau: casts and type arguments are erased before evaluation.
        Expression::TypeCast(cast) => literal_kind(&cast.expr),
        Expression::Suffixed(chain) => literal_kind(instantiated_expr(chain)?),
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => None,
    }
}

fn number_literal_kind(literal: &Literal) -> LiteralKind {
    if matches!(parse_lua_number(&literal.text, NumberSubtypes::IntFloat), Some(LuaNumber::Int(_)))
    {
        LiteralKind::Integer
    } else {
        LiteralKind::Float
    }
}

const fn unary_literal_kind(op: UnOp, operand: LiteralKind) -> Option<LiteralKind> {
    match op {
        UnOp::Not => Some(LiteralKind::Other),
        UnOp::Neg => {
            if operand.is_number() {
                Some(operand)
            } else {
                None
            }
        }
        UnOp::Len => {
            if matches!(operand, LiteralKind::String) {
                Some(LiteralKind::Integer)
            } else {
                None
            }
        }
        UnOp::BitNot => {
            if matches!(operand, LiteralKind::Integer) {
                Some(LiteralKind::Integer)
            } else {
                None
            }
        }
    }
}

/// Luau: the prefix a chain of nothing but instantiations gives its type
/// arguments to, which evaluation sees through.
fn instantiated_expr(chain: &SuffixedExpression) -> Option<&Expression> {
    chain
        .suffixes
        .iter()
        .all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }))
        .then_some(&chain.prefix)
}

/// Each operator applies in the order the level groups them.
fn run_literal_kind(run: &BinaryOp) -> Option<LiteralKind> {
    match run.precedence().1 {
        Associativity::Left => {
            let mut kind = literal_kind(&run.first)?;
            for link in &run.links {
                let operand = literal_kind(&link.operand)?;
                kind = binary_literal_kind(kind, link.op, operand, &link.operand)?;
            }
            Some(kind)
        }
        Associativity::Right => {
            let (last, before_last) = run.links.split_last()?;
            let mut kind = literal_kind(&last.operand)?;
            let left_operands =
                before_last.iter().rev().map(|link| &link.operand).chain(iter::once(&run.first));
            for (link, left_operand) in run.links.iter().rev().zip(left_operands) {
                kind =
                    binary_literal_kind(literal_kind(left_operand)?, link.op, kind, &link.operand)?;
            }
            Some(kind)
        }
    }
}

/// `rhs` is the right operand as written, which for a right-associative
/// operator is the start of the rest of its run.
fn binary_literal_kind(
    left: LiteralKind,
    op: BinOp,
    right: LiteralKind,
    rhs: &Expression,
) -> Option<LiteralKind> {
    let are_numbers = left.is_number() && right.is_number();
    let are_integers = left == LiteralKind::Integer && right == LiteralKind::Integer;
    let arithmetic_kind = if are_integers { LiteralKind::Integer } else { LiteralKind::Float };
    match op {
        BinOp::And | BinOp::Or => Some(if left == right { left } else { LiteralKind::Other }),
        BinOp::Eq | BinOp::Ne => Some(LiteralKind::Other),
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let are_strings = left == LiteralKind::String && right == LiteralKind::String;
            (are_numbers || are_strings).then_some(LiteralKind::Other)
        }
        BinOp::Concat => {
            (left.is_concatenable() && right.is_concatenable()).then_some(LiteralKind::String)
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul => are_numbers.then_some(arithmetic_kind),
        BinOp::Div | BinOp::Pow => are_numbers.then_some(LiteralKind::Float),
        // `%` and `//` raise on a zero integer divisor in 5.3+.
        BinOp::Mod | BinOp::FloorDiv => {
            (are_numbers && is_nonzero_number_literal(rhs)).then_some(arithmetic_kind)
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            are_integers.then_some(LiteralKind::Integer)
        }
    }
}

fn is_nonzero_number_literal(expr: &Expression) -> bool {
    let Expression::Number(literal) = unwrap_parentheses(expr) else {
        return false;
    };
    match parse_lua_number(&literal.text, NumberSubtypes::IntFloat) {
        Some(LuaNumber::Int(value)) => value != 0,
        Some(LuaNumber::Float(value)) => value != 0.0f64,
        None => false,
    }
}

/// A name read may be a global's, which fires `__index` on a metatabled
/// environment, so only literals, closures, `...`, and tables built from them
/// qualify.
pub(crate) fn is_pure_expr(expr: &Expression) -> bool {
    is_effect_free(expr, false)
}

/// [`is_pure_expr`], except that a name read counts: for a caller that keeps
/// every read in its original order, so `__index` can't observe a change.
pub(crate) fn is_pure_modulo_name_reads(expr: &Expression) -> bool {
    is_effect_free(expr, true)
}

fn is_effect_free(expr: &Expression, can_read_names: bool) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_) // Luau integers are exact literal values.
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::FunctionDef(_)
        // `...` can't be reassigned, so its value is fixed for the whole
        // function body.
        | Expression::VarArg(_) => true,
        Expression::Parenthesized(paren) => {
            is_effect_free(unwrap_parentheses(&paren.expr), can_read_names)
        }
        Expression::UnaryOp(_) => literal_kind(expr).is_some(),
        Expression::BinaryOp(binary_op) => {
            if is_logical(binary_op) {
                // and/or never invoke metamethods, so they are pure whenever
                // every operand is.
                is_effect_free(&binary_op.first, can_read_names)
                    && binary_op
                        .links
                        .iter()
                        .all(|link| is_effect_free(&link.operand, can_read_names))
            } else {
                literal_kind(expr).is_some()
            }
        }
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Bracketed { key, value, .. } => {
                is_valid_table_key(key) && is_effect_free(value, can_read_names)
            }
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_effect_free(value, can_read_names)
            }
        }),
        Expression::Name(_) => can_read_names,
        // A field access or an index can raise on a nil prefix or fire
        // `__index`, and a call can do anything. Luau's explicit type
        // arguments aren't evaluated at runtime.
        Expression::Suffixed(chain) => {
            instantiated_expr(chain).is_some_and(|instantiated| is_effect_free(instantiated, can_read_names))
        }
        // Luau type casts are transparent wrappers, so purity is the inner
        // expression's.
        Expression::TypeCast(cast) => {
            is_effect_free(&cast.expr, can_read_names)
        }
        Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `key` is a table constructor `[key]` that can't raise: `nil` and
/// NaN raise, and only a key spelled out as a literal is known to be neither.
fn is_valid_table_key(key: &Expression) -> bool {
    match unwrap_parentheses(key) {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::True(_)
        | Expression::False(_) => true,
        Expression::UnaryOp(unop) => {
            unop.op == UnOp::Neg
                && matches!(unwrap_parentheses(&unop.operand), Expression::Number(_))
        }
        Expression::Parenthesized(_)
        | Expression::Nil(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::BinaryOp(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `expr` is a literal, or operators over literals, that can't raise:
/// neither its value nor its evaluation depends on where it runs, so a pass
/// can move it.
pub(crate) fn is_closed_literal_expr(expr: &Expression) -> bool {
    literal_kind(expr).is_some()
}

/// How many levels of nesting a rewrite may add, so a run of rewrites can't
/// trade a flat program for one nested to the parser's cap.
pub(crate) const MAX_ADDED_DEPTH: usize = 32;

/// Whether `expr` still nests within the parser's cap once it sits
/// `added_levels` below `depth`, the level a pass's walk has reached.
///
/// The output has to reparse, so a rewrite that moves an expression deeper
/// asks first.
pub(crate) fn fits_at_depth(depth: usize, added_levels: usize, expr: &Expression) -> bool {
    depth.saturating_add(added_levels).saturating_add(expr_nesting_depth(expr)) <= MAX_NESTING_DEPTH
}

/// Whether `expr` evaluates into exactly one register with no temporaries.
pub(crate) const fn is_single_register_value(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_) => true,
        Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Whether the expression is guaranteed truthy at runtime. Only `nil` and
/// `false` are falsy in Lua, so numbers, strings, tables, and functions pass.
pub(crate) fn is_always_truthy(expr: &Expression) -> bool {
    match expr {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::True(_) => true,
        Expression::Parenthesized(paren) => is_always_truthy(unwrap_parentheses(&paren.expr)),
        Expression::UnaryOp(_) | Expression::BinaryOp(_) => {
            literal_kind(expr).is_some_and(|kind| kind.is_number() || kind == LiteralKind::String)
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Lua's integer-to-float conversion: C's `(double)` cast, which rounds to
/// the nearest representable value.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "the C cast Lua itself uses, and i64 has no From into f64"
)]
pub(crate) const fn int_to_float(value: i64) -> f64 {
    value as f64
}

pub(crate) fn parenthesize(expr: Expression) -> Expression {
    Expression::Parenthesized(Box::new(ParenthesizedExpression { span: Span::default(), expr }))
}

/// Parenthesizes a call or `...` that replaces an `and`/`or`, which always
/// yields one value.
pub(crate) fn truncate_multi_value(expr: Expression) -> Expression {
    if is_multi_value(&expr) { parenthesize(expr) } else { expr }
}

/// Whether `run`, printed bare under a unary operator, regroups. Every
/// binary operator but `^` binds looser than a unary one.
pub(crate) fn binds_looser_than_unary(run: &BinaryOp) -> bool {
    run.precedence().0 < UnOp::PRECEDENCE
}

/// The side of its operator that an operand between two of `run`'s
/// operators sits on, in the pairs the run groups into. The first operand is
/// always a left operand and the last always a right one.
pub(crate) fn inner_operand_side(run: &BinaryOp) -> OperandSide {
    match run.precedence().1 {
        Associativity::Left => OperandSide::Right,
        Associativity::Right => OperandSide::Left,
    }
}

/// Whether `run` is `and` or `or`, which no metamethod can intercept.
pub(crate) fn is_logical(run: &BinaryOp) -> bool {
    matches!(run.links.first().op, BinOp::And | BinOp::Or)
}

/// Whether `expr` prints a Luau if-expression last, whose `else` branch would
/// take in an operator printed after it.
pub(crate) fn ends_in_if_expr(expr: &Expression) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            Expression::IfExpression(_) => return true,
            Expression::BinaryOp(binary_op) => &binary_op.links.last().operand,
            Expression::UnaryOp(unop) => &unop.operand,
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return false,
        };
    }
}

/// Whether `inner` printed bare as the left operand of `outer` reparses as
/// that same operand.
pub(crate) fn can_unwrap_in_binop_lhs(inner: &Expression, outer: BinOp) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        // A call keeps its parens, though `+` truncates it anyway.
        Expression::Suffixed(chain) => chain.is_var(),
        // Unary ops bind looser than ^, so (-a)^b needs its parens.
        Expression::UnaryOp(_) => outer != BinOp::Pow && !ends_in_if_expr(inner),
        Expression::BinaryOp(binary_op) => {
            binary_op.can_nest_bare(outer, OperandSide::Left) && !ends_in_if_expr(inner)
        }
        Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Whether `inner` printed bare as the right operand of `outer` reparses as
/// that same operand.
pub(crate) fn can_unwrap_in_binop_rhs(inner: &Expression, outer: BinOp) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        // A call keeps its parens, though `+` truncates it anyway.
        Expression::Suffixed(chain) => chain.is_var(),
        Expression::UnaryOp(_) => !ends_in_if_expr(inner),
        Expression::BinaryOp(binary_op) => {
            binary_op.can_nest_bare(outer, OperandSide::Right) && !ends_in_if_expr(inner)
        }
        Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}
