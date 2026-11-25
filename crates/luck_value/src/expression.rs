//! One analysis of what evaluating an expression does, which every question
//! the crate answers about an expression reads, so the answers agree. A
//! question that settles on less, a value or literalness, stops once it has,
//! and reads the analysis only for the parts that need all of it.

use std::iter;

use luck_ast::{BinaryOp, Expression, Field, IfExpression, Suffix, TableConstructor};
use luck_token::{Associativity, BinOp, UnOp};

use crate::context::ValueContext;
use crate::value::ConstantValue;

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
    /// The value the expression evaluates to, in Luau's exact arithmetic,
    /// when that's known and evaluating it can't raise, run a metamethod, or
    /// read a global.
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
        pure_value(self, context)
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
        !holds_variable_or_fresh_value(self) && analyze(self, context).is_literal
    }
}

/// The type of a value an expression yields, as far as the analysis tells.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueType {
    Nil,
    Boolean,
    Number,
    String,
    Table,
    Function,
    /// Any type: a variable's value, or a metamethod's result.
    Undetermined,
}

impl ValueType {
    const fn of(value: ConstantValue<'_>) -> Self {
        match value {
            ConstantValue::Nil => Self::Nil,
            ConstantValue::Boolean(_) => Self::Boolean,
            ConstantValue::Number(_) => Self::Number,
            ConstantValue::String(_) => Self::String,
        }
    }

    /// Whether a value of this type is truthy, when the type tells.
    const fn truthiness(self) -> Option<bool> {
        match self {
            Self::Nil => Some(false),
            Self::Number | Self::String | Self::Table | Self::Function => Some(true),
            Self::Boolean | Self::Undetermined => None,
        }
    }

    /// Whether a value of this type may be a table or userdata, the only
    /// values whose `__eq` runs, and only when both operands are.
    const fn may_have_eq_metamethod(self) -> bool {
        match self {
            Self::Table | Self::Undetermined => true,
            Self::Nil | Self::Boolean | Self::Number | Self::String | Self::Function => false,
        }
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
            ty: ValueType::of(value),
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

    /// What `self` yields, with the effects and the literalness of evaluating
    /// both `self` and `other`.
    const fn alongside(self, other: Self) -> Self {
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
            ..self.alongside(other)
        }
    }

    /// Whether a table constructor takes the value as a key, which it does
    /// for any but `nil` and NaN.
    const fn is_valid_key(self) -> bool {
        match self.value {
            Some(ConstantValue::Nil) => false,
            Some(ConstantValue::Number(number)) => !number.is_nan(),
            Some(ConstantValue::Boolean(_) | ConstantValue::String(_)) => true,
            None => matches!(
                self.ty,
                ValueType::Boolean | ValueType::String | ValueType::Table | ValueType::Function
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
        Expression::StringLiteral(literal) => {
            Outcome::constant(ConstantValue::String(literal.value.as_bytes()))
        }
        // Text alone is a string constant, and an expression in it calls the
        // `format` method a string finds through its metatable.
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
        // A chain of type instantiations alone runs nothing, since types
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
        Expression::UnaryOp(unary) => unary_outcome(unary.op, analyze(&unary.operand, context)),
        Expression::BinaryOp(run) => run_outcome(run, context),
        Expression::IfExpression(if_expr) => if_outcome(if_expr, context),
        // A cast runs nothing.
        Expression::TypeCast(cast) => analyze(&cast.expr, context),
        Expression::Error(_) => Outcome::OPAQUE,
    }
}

/// The value [`analyze`] finds `expr` yields when it finds no effect, read
/// only as far as the value needs.
///
/// An operator other than `and`, `or`, and `not` has a value only when
/// every operand has one and no effect, and on known values
/// [`ConstantValue::binary`] has a result only where the operator can't
/// raise or run a metamethod, so the first operand with no value settles
/// it. A name, `...`, a table, and a closure never have one, and the rest,
/// which read their operands' truthiness, take the whole analysis.
fn pure_value<'ast>(
    expr: &Expression<'ast>,
    context: &dyn ValueContext,
) -> Option<ConstantValue<'ast>> {
    match expr {
        Expression::Nil(_) => Some(ConstantValue::Nil),
        Expression::False(_) => Some(ConstantValue::Boolean(false)),
        Expression::True(_) => Some(ConstantValue::Boolean(true)),
        Expression::Number(literal) => Some(ConstantValue::Number(literal.value)),
        Expression::StringLiteral(literal) => Some(ConstantValue::String(literal.value.as_bytes())),
        Expression::InterpolatedString(string) => {
            string.segments.is_empty().then(|| ConstantValue::String(string.tail.value.as_bytes()))
        }
        Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::Error(_) => None,
        Expression::Suffixed(chain) => {
            if chain
                .suffixes
                .iter()
                .all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }))
            {
                pure_value(&chain.prefix, context)
            } else {
                None
            }
        }
        Expression::Parenthesized(parenthesized) => pure_value(&parenthesized.expr, context),
        Expression::TypeCast(cast) => pure_value(&cast.expr, context),
        Expression::UnaryOp(unary) => match unary.op {
            UnOp::Neg | UnOp::Len => {
                let operand = Outcome::constant(pure_value(&unary.operand, context)?);
                unary_outcome(unary.op, operand).value
            }
            UnOp::Not => whole_analysis_value(expr, context),
        },
        Expression::BinaryOp(run) => {
            if run.links.iter().any(|link| matches!(link.op, BinOp::And | BinOp::Or)) {
                return whole_analysis_value(expr, context);
            }
            match run.precedence().1 {
                Associativity::Left => {
                    run.links.iter().try_fold(pure_value(&run.first, context)?, |lhs, link| {
                        lhs.binary(link.op, pure_value(&link.operand, context)?)
                    })
                }
                Associativity::Right => {
                    let operands =
                        iter::once(&run.first).chain(run.links.iter().map(|link| &link.operand));
                    let left_operands = operands.rev().skip(1);
                    let last = pure_value(&run.links.last().operand, context)?;
                    run.links.iter().rev().zip(left_operands).try_fold(last, |rhs, (link, lhs)| {
                        pure_value(lhs, context)?.binary(link.op, rhs)
                    })
                }
            }
        }
        Expression::IfExpression(_) => whole_analysis_value(expr, context),
    }
}

/// Whether `expr` holds, anywhere, even in an operand that never runs, a
/// part [`analyze`] finds is no literal whatever surrounds it: a variable,
/// `...`, a table, a closure, a call, an index, or text with an expression
/// in it. Literalness only ever narrows as the analysis joins its parts, so
/// one such part settles it, and the search stops there.
fn holds_variable_or_fresh_value(expr: &Expression<'_>) -> bool {
    match expr {
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::StringLiteral(_) => false,
        Expression::InterpolatedString(string) => !string.segments.is_empty(),
        Expression::VarArg(_)
        | Expression::Name(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::Error(_) => true,
        Expression::Suffixed(chain) => {
            !chain.suffixes.iter().all(|suffix| matches!(suffix, Suffix::TypeInstantiation { .. }))
                || holds_variable_or_fresh_value(&chain.prefix)
        }
        Expression::Parenthesized(parenthesized) => {
            holds_variable_or_fresh_value(&parenthesized.expr)
        }
        Expression::TypeCast(cast) => holds_variable_or_fresh_value(&cast.expr),
        Expression::UnaryOp(unary) => holds_variable_or_fresh_value(&unary.operand),
        Expression::BinaryOp(run) => {
            holds_variable_or_fresh_value(&run.first)
                || run.links.iter().any(|link| holds_variable_or_fresh_value(&link.operand))
        }
        Expression::IfExpression(if_expr) => {
            holds_variable_or_fresh_value(&if_expr.condition)
                || holds_variable_or_fresh_value(&if_expr.then_expr)
                || if_expr.elseif_clauses.iter().any(|clause| {
                    holds_variable_or_fresh_value(&clause.condition)
                        || holds_variable_or_fresh_value(&clause.expr)
                })
                || holds_variable_or_fresh_value(&if_expr.else_expr)
        }
    }
}

fn whole_analysis_value<'ast>(
    expr: &Expression<'ast>,
    context: &dyn ValueContext,
) -> Option<ConstantValue<'ast>> {
    let outcome = analyze(expr, context);
    outcome.value.filter(|_| !outcome.has_effects)
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

fn unary_outcome(op: UnOp, operand: Outcome<'_>) -> Outcome<'_> {
    let (number, is_safe) = match op {
        // `not` runs no metamethod.
        UnOp::Not => {
            return Outcome {
                ty: ValueType::Boolean,
                value: operand.truthiness.map(|is_truthy| ConstantValue::Boolean(!is_truthy)),
                truthiness: operand.truthiness.map(|is_truthy| !is_truthy),
                ..operand
            };
        }
        UnOp::Neg => {
            let negated = operand.value.and_then(ConstantValue::as_number).map(|number| -number);
            (negated, operand.ty == ValueType::Number)
        }
        // A string's length is its byte count, and a table's may run `__len`.
        UnOp::Len => {
            let length = match operand.value {
                Some(ConstantValue::String(bytes)) => {
                    u32::try_from(bytes.len()).ok().map(f64::from)
                }
                Some(ConstantValue::Nil | ConstantValue::Boolean(_) | ConstantValue::Number(_))
                | None => None,
            };
            (length, operand.ty == ValueType::String)
        }
    };
    operand.operation(number_or_any(is_safe), number.map(ConstantValue::Number), is_safe)
}

/// A number, or what a metamethod returns, which may be anything.
const fn number_or_any(is_number: bool) -> ValueType {
    if is_number { ValueType::Number } else { ValueType::Undetermined }
}

fn run_outcome<'ast>(run: &BinaryOp<'ast>, context: &dyn ValueContext) -> Outcome<'ast> {
    match run.precedence().1 {
        Associativity::Left => run.links.iter().fold(analyze(&run.first, context), |lhs, link| {
            binary_outcome(lhs, link.op, analyze(&link.operand, context))
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
                    binary_outcome(analyze(lhs, context), link.op, rhs)
                })
        }
    }
}

fn binary_outcome<'ast>(lhs: Outcome<'ast>, op: BinOp, rhs: Outcome<'ast>) -> Outcome<'ast> {
    let are_numbers = lhs.ty == ValueType::Number && rhs.ty == ValueType::Number;
    let (ty, is_safe) = match op {
        BinOp::And | BinOp::Or => return logical_outcome(lhs, rhs, op == BinOp::Or),
        BinOp::Eq | BinOp::Ne => {
            let is_safe = !(lhs.ty.may_have_eq_metamethod() && rhs.ty.may_have_eq_metamethod());
            (ValueType::Boolean, is_safe)
        }
        // Luau makes a boolean of what an order metamethod returns.
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let are_strings = lhs.ty == ValueType::String && rhs.ty == ValueType::String;
            (ValueType::Boolean, are_numbers || are_strings)
        }
        BinOp::Concat => {
            let is_joinable = |ty| matches!(ty, ValueType::Number | ValueType::String);
            let is_safe = is_joinable(lhs.ty) && is_joinable(rhs.ty);
            (if is_safe { ValueType::String } else { ValueType::Undetermined }, is_safe)
        }
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow => (number_or_any(are_numbers), are_numbers),
    };
    let value = lhs.value.zip(rhs.value).and_then(|(left, right)| left.binary(op, right));
    lhs.alongside(rhs).operation(ty, value, is_safe)
}

/// `lhs and rhs`, or with `is_or`, `lhs or rhs`, neither of which runs a
/// metamethod.
fn logical_outcome<'ast>(lhs: Outcome<'ast>, rhs: Outcome<'ast>, is_or: bool) -> Outcome<'ast> {
    match lhs.truthiness {
        // `false and x` and `true or x` yield their left operand, and the
        // right one never runs.
        Some(is_truthy) if is_truthy == is_or => {
            Outcome { is_literal: lhs.is_literal && rhs.is_literal, ..lhs }
        }
        // `true and x` and `false or x` yield their right operand.
        Some(_) => rhs.alongside(lhs),
        None => Outcome {
            // `x and false` is falsy, and `x or true` truthy, whichever operand
            // it yields.
            truthiness: rhs.truthiness.filter(|&is_truthy| is_truthy == is_or),
            ..lhs.either(rhs)
        },
    }
}

/// `if c then a elseif d then b else e`, which runs its conditions in
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

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::{Name, Visit};
    use luck_testgen::{generate_full_grammar, generate_runtime_safe, source_fixtures};

    use super::*;
    use crate::context::NoBindings;

    /// A context in which every name is a local, so only operators and
    /// indexes have effects.
    struct NoGlobals;

    impl ValueContext for NoGlobals {
        fn is_global_reference(&self, _name: &Name<'_>) -> bool {
            false
        }
    }

    /// Checks every expression of a tree against the whole analysis, and
    /// counts the ones with a value and the literal ones.
    struct Agreement {
        valued: usize,
        literal: usize,
    }

    impl<'ast> Visit<'ast> for Agreement {
        fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
            let contexts: [&dyn ValueContext; 2] = [&NoBindings, &NoGlobals];
            for context in contexts {
                let value = pure_value(expr, context);
                assert_eq!(value, whole_analysis_value(expr, context), "{expr:?}");
                self.valued += usize::from(value.is_some());
                let is_literal = expr.is_literal_value(context);
                assert_eq!(is_literal, analyze(expr, context).is_literal, "{expr:?}");
                self.literal += usize::from(is_literal);
            }
            self.walk_expr(expr);
        }
    }

    #[test]
    fn each_question_answers_as_the_whole_analysis_does() {
        let generated = (0..40)
            .flat_map(|seed| [generate_full_grammar(seed, 25), generate_runtime_safe(seed, 25)]);
        let sources = source_fixtures().into_iter().map(|fixture| fixture.source).chain(generated);
        let mut agreement = Agreement { valued: 0, literal: 0 };
        for source in sources {
            let allocator = Allocator::new();
            let parsed = luck_parser::parse(&allocator, source.as_bytes());
            agreement.visit_block(&parsed.chunk);
        }
        let Agreement { valued, literal } = agreement;
        assert!(valued > 1_000 && literal > 1_000, "{valued} with a value, {literal} literal");
    }
}
