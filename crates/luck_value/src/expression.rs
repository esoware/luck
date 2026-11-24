//! One analysis of what evaluating an expression does, which every question
//! the crate answers about an expression reads, so the answers agree.

use std::iter;

use luck_ast::{BinaryOp, Expression, Field, IfExpression, Suffix, TableConstructor};
use luck_token::{Associativity, BinOp, LuaNumber, LuaVersion, UnOp};

use crate::arithmetic::negate;
use crate::context::ValueContext;
use crate::value::{ConstantValue, ValueType};

/// Whether an expression's value is truthy, which every value but `nil` and
/// `false` is.
pub trait ToBoolean {
    /// Whether the value the expression yields when its evaluation completes
    /// is truthy, when that's known. Evaluating it may still raise, run a
    /// metamethod, or read a global: `{f()}` is truthy.
    fn to_boolean(&self, context: &dyn ValueContext) -> Option<bool>;
}

/// The value an expression evaluates to.
pub trait ConstantEvaluation<'ast> {
    /// The value the expression evaluates to, in the target's exact
    /// arithmetic, when that's known and evaluating it can't raise, run a
    /// metamethod, or read a global.
    fn evaluate_value(&self, context: &dyn ValueContext) -> Option<ConstantValue<'ast>>;

    /// Whether the expression is truthy, when that's known and evaluating it
    /// can't raise, run a metamethod, or read a global, so it can go: `{}`
    /// is truthy, though it has no constant value.
    fn get_side_free_boolean_value(&self, context: &dyn ValueContext) -> Option<bool>;
}

/// Whether evaluating an expression does anything a program can observe.
pub trait MayHaveSideEffects {
    /// Whether evaluating the expression may raise, run a metamethod, or read
    /// a global. Every metamethod is assumed to do anything, so an operator
    /// is pure only on values whose types rule one out, and an index or a call
    /// never is.
    fn may_have_side_effects(&self, context: &dyn ValueContext) -> bool;
}

/// Whether an expression evaluates to the same value wherever it is.
pub trait IsLiteralValue {
    /// Whether the expression evaluates to the same value wherever and
    /// whenever it runs, so it can move: it can't raise or run a metamethod,
    /// and reads no variable and no `...` and builds no table or closure,
    /// even in an operand that never runs.
    fn is_literal_value(&self, context: &dyn ValueContext) -> bool;
}

impl ToBoolean for Expression<'_> {
    fn to_boolean(&self, context: &dyn ValueContext) -> Option<bool> {
        analyze(self, context).truthiness
    }
}

impl<'ast> ConstantEvaluation<'ast> for Expression<'ast> {
    fn evaluate_value(&self, context: &dyn ValueContext) -> Option<ConstantValue<'ast>> {
        let outcome = analyze(self, context);
        outcome.value.filter(|_| !outcome.has_effects)
    }

    fn get_side_free_boolean_value(&self, context: &dyn ValueContext) -> Option<bool> {
        let outcome = analyze(self, context);
        outcome.truthiness.filter(|_| !outcome.has_effects)
    }
}

impl MayHaveSideEffects for Expression<'_> {
    fn may_have_side_effects(&self, context: &dyn ValueContext) -> bool {
        analyze(self, context).has_effects
    }
}

impl IsLiteralValue for Expression<'_> {
    fn is_literal_value(&self, context: &dyn ValueContext) -> bool {
        analyze(self, context).is_literal
    }
}

/// What evaluating an expression is known to do.
#[derive(Clone, Copy)]
struct Outcome<'ast> {
    /// The type of the value it yields when it completes.
    ty: ValueType,
    /// That value, when it's known.
    value: Option<ConstantValue<'ast>>,
    /// Whether that value is truthy, when it's known.
    truthiness: Option<bool>,
    /// Whether it may raise, run a metamethod, or read a global.
    has_effects: bool,
    /// Whether it yields the same value wherever and whenever it runs, which
    /// one with effects never does.
    is_literal: bool,
}

impl<'ast> Outcome<'ast> {
    /// What every expression the analysis doesn't see into, a call or an
    /// index, yields: anything, after doing anything.
    const OPAQUE: Self = Self::variable(true);

    const fn constant(value: ConstantValue<'ast>) -> Self {
        Self {
            ty: value.ty(),
            value: Some(value),
            truthiness: Some(value.to_boolean()),
            has_effects: false,
            is_literal: true,
        }
    }

    /// A variable's value or `...`, which may be anything.
    const fn variable(has_effects: bool) -> Self {
        Self {
            ty: ValueType::Undetermined,
            value: None,
            truthiness: None,
            has_effects,
            is_literal: false,
        }
    }

    /// A table or a closure, which is a new value each time it's built.
    const fn fresh(ty: ValueType, has_effects: bool) -> Self {
        Self { ty, value: None, truthiness: ty.truthiness(), has_effects, is_literal: false }
    }

    /// An operator's result on operands whose outcomes `self` joins: a value
    /// of type `ty`, which is `result` when that's known. An operator that
    /// isn't `is_safe` may raise or run a metamethod, so its value isn't
    /// known.
    fn operation(self, ty: ValueType, result: Option<ConstantValue<'ast>>, is_safe: bool) -> Self {
        let value = result.filter(|_| is_safe);
        Self {
            ty,
            value,
            truthiness: value.map(ConstantValue::to_boolean).or_else(|| ty.truthiness()),
            has_effects: self.has_effects || !is_safe,
            is_literal: self.is_literal && is_safe,
        }
    }

    /// The effects and the literalness of evaluating `self` and `other`.
    const fn and(self, other: Self) -> Self {
        Self {
            has_effects: self.has_effects || other.has_effects,
            is_literal: self.is_literal && other.is_literal,
            ..self
        }
    }

    /// What an expression that yields `self` or `other` yields, not knowing
    /// which.
    fn either(self, other: Self) -> Self {
        Self {
            ty: if self.ty == other.ty { self.ty } else { ValueType::Undetermined },
            value: self.value.filter(|_| self.value == other.value),
            truthiness: self.truthiness.filter(|_| self.truthiness == other.truthiness),
            ..self.and(other)
        }
    }

    /// The integer the value converts to for a bitwise operator, when it's a
    /// number with an integer's value.
    fn integer(self) -> Option<i64> {
        self.value?.as_number()?.to_integer()
    }

    /// Whether a table constructor takes the value as a key, which it does
    /// for any but `nil` and NaN.
    const fn is_valid_key(self) -> bool {
        match self.value {
            Some(ConstantValue::Nil) => false,
            Some(ConstantValue::Number(LuaNumber::Float(float))) => !float.is_nan(),
            Some(
                ConstantValue::Boolean(_)
                | ConstantValue::Number(LuaNumber::Int(_))
                | ConstantValue::Integer(_)
                | ConstantValue::String(_),
            ) => true,
            None => matches!(
                self.ty,
                ValueType::Boolean
                    | ValueType::String
                    | ValueType::Table
                    | ValueType::Function
                    | ValueType::Integer
            ),
        }
    }
}

fn analyze<'ast>(expr: &Expression<'ast>, context: &dyn ValueContext) -> Outcome<'ast> {
    match expr {
        Expression::Nil(_) => Outcome::constant(ConstantValue::Nil),
        Expression::False(_) => Outcome::constant(ConstantValue::Boolean(false)),
        Expression::True(_) => Outcome::constant(ConstantValue::Boolean(true)),
        Expression::Number(literal) => Outcome::constant(ConstantValue::Number(literal.value)),
        // Luau
        Expression::Integer(literal) => Outcome::constant(ConstantValue::Integer(literal.value)),
        Expression::StringLiteral(literal) => {
            Outcome::constant(ConstantValue::String(literal.value.as_bytes()))
        }
        // Luau: text alone is a string constant, and an expression in it
        // calls the `format` method a string finds through its metatable.
        Expression::InterpolatedString(string) => {
            if string.segments.is_empty() {
                Outcome::constant(ConstantValue::String(string.tail.value.as_bytes()))
            } else {
                Outcome::OPAQUE
            }
        }
        Expression::VarArg(_) => Outcome::variable(false),
        Expression::Name(name) => Outcome::variable(context.is_global_reference(name)),
        Expression::FunctionDef(_) => Outcome::fresh(ValueType::Function, false),
        Expression::TableConstructor(table) => table_outcome(table, context),
        // A chain of Luau instantiations alone runs nothing, since types
        // don't.
        Expression::Suffixed(chain) => {
            if chain
                .suffixes
                .iter()
                .all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }))
            {
                analyze(&chain.prefix, context)
            } else {
                Outcome::OPAQUE
            }
        }
        // Taking the first value of one changes nothing the analysis tells.
        Expression::Parenthesized(parenthesized) => analyze(&parenthesized.expr, context),
        Expression::UnaryOp(unary) => {
            unary_outcome(unary.op, analyze(&unary.operand, context), context.version())
        }
        Expression::BinaryOp(run) => run_outcome(run, context),
        // Luau
        Expression::IfExpression(if_expr) => if_outcome(if_expr, context),
        // Luau: a cast doesn't run.
        Expression::TypeCast(cast) => analyze(&cast.expr, context),
        Expression::Error(_) => Outcome::OPAQUE,
    }
}

fn table_outcome<'ast>(
    table: &TableConstructor<'ast>,
    context: &dyn ValueContext,
) -> Outcome<'ast> {
    let has_effects = table.fields.iter().any(|field| match field {
        Field::Positional(value) | Field::Named { value, .. } => {
            analyze(value, context).has_effects
        }
        Field::Bracketed { key, value, .. } => {
            let key_outcome = analyze(key, context);
            key_outcome.has_effects
                || !key_outcome.is_valid_key()
                || analyze(value, context).has_effects
        }
    });
    Outcome::fresh(ValueType::Table, has_effects)
}

fn unary_outcome(op: UnOp, operand: Outcome<'_>, version: LuaVersion) -> Outcome<'_> {
    match op {
        // `not` runs no metamethod.
        UnOp::Not => Outcome {
            ty: ValueType::Boolean,
            value: operand.truthiness.map(|is_truthy| ConstantValue::Boolean(!is_truthy)),
            truthiness: operand.truthiness.map(|is_truthy| !is_truthy),
            ..operand
        },
        UnOp::Neg => {
            let is_safe = operand.ty == ValueType::Number;
            let value = operand.value.and_then(ConstantValue::as_number).map(negate);
            operand.operation(number_or_any(is_safe), value.map(ConstantValue::Number), is_safe)
        }
        // A string's length is its byte count, and a table's may run `__len`.
        UnOp::Len => {
            let length = match operand.value {
                Some(ConstantValue::String(bytes)) => i64::try_from(bytes.len()).ok(),
                Some(
                    ConstantValue::Nil
                    | ConstantValue::Boolean(_)
                    | ConstantValue::Number(_)
                    | ConstantValue::Integer(_),
                )
                | None => None,
            };
            let value = length.map(|integer| {
                let number = LuaNumber::Int(integer);
                if version.has_integer_subtype() {
                    number
                } else {
                    LuaNumber::Float(number.to_f64())
                }
            });
            let is_safe = operand.ty == ValueType::String;
            operand.operation(number_or_any(is_safe), value.map(ConstantValue::Number), is_safe)
        }
        // Lua 5.3+: a float with a fraction raises, as a string may.
        UnOp::BitNot => {
            let integer = operand.integer().filter(|_| version.has_bitwise_ops());
            let value = integer.map(|bits| ConstantValue::Number(LuaNumber::Int(!bits)));
            operand.operation(number_or_any(integer.is_some()), value, integer.is_some())
        }
    }
}

/// A number, or what a metamethod returns, which may be anything.
const fn number_or_any(is_number: bool) -> ValueType {
    if is_number { ValueType::Number } else { ValueType::Undetermined }
}

fn run_outcome<'ast>(run: &BinaryOp<'ast>, context: &dyn ValueContext) -> Outcome<'ast> {
    let version = context.version();
    match run.precedence().1 {
        Associativity::Left => run.links.iter().fold(analyze(&run.first, context), |lhs, link| {
            binary_outcome(lhs, link.op, analyze(&link.operand, context), version)
        }),
        // `..` and `^` group from the right, so `a .. b .. c` is
        // `a .. (b .. c)`.
        Associativity::Right => {
            let operands = iter::once(&run.first).chain(run.links.iter().map(|link| &link.operand));
            let left_operands = operands.rev().skip(1);
            run.links
                .iter()
                .rev()
                .zip(left_operands)
                .fold(analyze(&run.links.last().operand, context), |rhs, (link, lhs)| {
                    binary_outcome(analyze(lhs, context), link.op, rhs, version)
                })
        }
    }
}

fn binary_outcome<'ast>(
    lhs: Outcome<'ast>,
    op: BinOp,
    rhs: Outcome<'ast>,
    version: LuaVersion,
) -> Outcome<'ast> {
    let operands = lhs.and(rhs);
    let value = lhs.value.zip(rhs.value).and_then(|(left, right)| left.binary(op, right, version));
    let are_numbers = lhs.ty == ValueType::Number && rhs.ty == ValueType::Number;
    match op {
        BinOp::And => logical_outcome(lhs, rhs, false),
        BinOp::Or => logical_outcome(lhs, rhs, true),
        BinOp::Eq | BinOp::Ne => {
            let is_safe = !(lhs.ty.may_have_eq_metamethod() && rhs.ty.may_have_eq_metamethod());
            operands.operation(ValueType::Boolean, value, is_safe)
        }
        // Lua makes a boolean of what an order metamethod returns.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let are_strings = lhs.ty == ValueType::String && rhs.ty == ValueType::String;
            operands.operation(ValueType::Boolean, value, are_numbers || are_strings)
        }
        BinOp::Concat => {
            let is_joinable = |ty| matches!(ty, ValueType::Number | ValueType::String);
            let is_safe = is_joinable(lhs.ty) && is_joinable(rhs.ty);
            let ty = if is_safe { ValueType::String } else { ValueType::Undetermined };
            operands.operation(ty, value, is_safe)
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
            operands.operation(number_or_any(are_numbers), value, are_numbers)
        }
        // Lua 5.3+ raises on an integer divided by zero.
        BinOp::FloorDiv | BinOp::Mod => {
            let is_safe = are_numbers
                && !(version.has_integer_subtype() && may_divide_integer_by_zero(lhs, rhs));
            operands.operation(number_or_any(is_safe), value, is_safe)
        }
        // Lua 5.3+: a float with a fraction raises, as a string may.
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
            let is_safe =
                version.has_bitwise_ops() && lhs.integer().is_some() && rhs.integer().is_some();
            operands.operation(number_or_any(is_safe), value, is_safe)
        }
    }
}

/// Whether two numbers may both be integers with the second zero, which
/// they can't be once either is known to be a float or the divisor nonzero.
fn may_divide_integer_by_zero(lhs: Outcome<'_>, rhs: Outcome<'_>) -> bool {
    match (
        lhs.value.and_then(ConstantValue::as_number),
        rhs.value.and_then(ConstantValue::as_number),
    ) {
        (Some(LuaNumber::Float(_)), _) | (_, Some(LuaNumber::Float(_))) => false,
        (_, Some(LuaNumber::Int(divisor))) => divisor == 0,
        (_, None) => true,
    }
}

/// `lhs and rhs`, or with `is_or`, `lhs or rhs`, neither of which runs a
/// metamethod.
fn logical_outcome<'ast>(lhs: Outcome<'ast>, rhs: Outcome<'ast>, is_or: bool) -> Outcome<'ast> {
    match lhs.truthiness {
        // `false and x` and `true or x` yield their left operand, and the
        // right one never runs.
        Some(is_truthy) if is_truthy == is_or => {
            Outcome { has_effects: lhs.has_effects, ..lhs.and(rhs) }
        }
        // `true and x` and `false or x` yield their right operand.
        Some(_) => rhs.and(lhs),
        None => Outcome {
            // `x and false` is falsy, and `x or true` truthy, whichever operand
            // it yields.
            truthiness: rhs.truthiness.filter(|&is_truthy| is_truthy == is_or),
            ..lhs.either(rhs)
        },
    }
}

/// Luau: `if c then a elseif d then b else e`, which runs its conditions in
/// order up to the first truthy one and yields that one's branch.
fn if_outcome<'ast>(if_expr: &IfExpression<'ast>, context: &dyn ValueContext) -> Outcome<'ast> {
    let arms = iter::once((&if_expr.condition, &if_expr.then_expr))
        .chain(if_expr.elseif_clauses.iter().map(|clause| (&clause.condition, &clause.expr)));
    let else_branch = analyze(&if_expr.else_expr, context);
    let mut is_literal = else_branch.is_literal;
    let mut conditions_have_effects = false;
    let mut yielded: Option<Outcome<'ast>> = None;
    let mut is_settled = false;
    for (condition_expr, branch_expr) in arms {
        let condition = analyze(condition_expr, context);
        let branch = analyze(branch_expr, context);
        is_literal &= condition.is_literal && branch.is_literal;
        if is_settled {
            continue;
        }
        conditions_have_effects |= condition.has_effects;
        if condition.truthiness != Some(false) {
            yielded = Some(yielded.map_or(branch, |others| others.either(branch)));
        }
        is_settled = condition.truthiness == Some(true);
    }
    let outcome = match yielded {
        Some(settled) if is_settled => settled,
        Some(others) => others.either(else_branch),
        None => else_branch,
    };
    Outcome { has_effects: conditions_have_effects || outcome.has_effects, is_literal, ..outcome }
}
