//! Folding operators whose operands evaluate, which `luck_value` does in the
//! target's exact arithmetic.
//!
//! A number folds only to a literal no longer than the operators it
//! replaces, as `oxc_minifier` folds one, since `1/3` and `1/0` spell longer
//! ones. A boolean or `nil` always folds, since a branch that tests it can go
//! too.

use luck_allocator::{Allocator, ThinBytes};
use luck_ast::{Expression, GetSpan as _, OneOrMore};
use luck_token::{BinOp, LuaVersion, Span, UnOp, spell_number};
use luck_value::{ConstantEvaluation as _, ConstantValue, MayHaveSideEffects as _, ToBoolean as _};

use crate::binding::BoundNames;
use crate::expr::truncate_multi_value;

/// Folds a run of binary operators or a unary operator, and says whether it
/// did. Hands `drop` each operand it drops.
pub(super) fn fold<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    match expr {
        Expression::BinaryOp(run) => match run.links.first().op {
            BinOp::And | BinOp::Or => fold_logical_run(allocator, context, expr, drop),
            BinOp::Concat => fold_concat_run(allocator, context, expr, drop),
            BinOp::Pow => fold_power_run(allocator, context, expr, drop),
            BinOp::Add
            | BinOp::Sub
            | BinOp::Mul
            | BinOp::Div
            | BinOp::FloorDiv
            | BinOp::Mod
            | BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr => fold_left_run(allocator, context, expr, drop),
        },
        Expression::UnaryOp(_) => fold_unary(allocator, context, expr, drop),
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
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

/// Folds the run's operators from the left while their operands evaluate,
/// as the pairs it groups into would, as far as the longest prefix whose
/// value spells no longer.
///
/// An operator that doesn't fold leaves a run on its left, which no later one
/// folds with either.
fn fold_left_run<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let version = context.0;
    let Some(mut value) = run.first.evaluate_value(&context) else {
        return false;
    };
    let mut bound = printed_length_bound(&run.first, version);
    // How many links fold, and into what.
    let mut folded = None;
    for (index, link) in run.links.iter().enumerate() {
        let Some(next) = link
            .operand
            .evaluate_value(&context)
            .and_then(|rhs| value.binary(link.op, rhs, version))
        else {
            break;
        };
        value = next;
        bound = joined_bound(bound, link.op, &link.operand, version);
        if is_short_enough(value, bound, version) {
            folded = Some((index.saturating_add(1), value));
        }
    }
    let Some((folded_links, folded_value)) = folded else {
        return false;
    };
    let folded_operands = run.links.get(..folded_links).unwrap_or_default();
    let span =
        folded_operands.iter().fold(run.first.span(), |span, link| span.merge(link.operand.span()));
    let Some(literal) = value_to_expr(allocator, folded_value, span) else {
        return false;
    };
    drop(&run.first);
    for link in folded_operands {
        drop(&link.operand);
    }
    match OneOrMore::from_list(run.links.split_off(folded_links)) {
        Some(rest) => {
            run.first = literal;
            run.links = rest;
        }
        None => *expr = literal,
    }
    true
}

/// Folds the run's `^`s from the right while their operands evaluate, as the
/// nested pairs it groups into would, as far as the longest suffix whose
/// value spells no longer.
fn fold_power_run<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let version = context.0;
    let last = &run.links.last().operand;
    let Some(mut value) = last.evaluate_value(&context) else {
        return false;
    };
    let mut bound = printed_length_bound(last, version);
    // The operand the folded value starts at, counting `run.first` as 0 and
    // each link's as one past the link's index, and the value.
    let mut folded = None;
    for (index, link) in run.links.iter().enumerate().rev() {
        let left = index.checked_sub(1).and_then(|i| run.links.get(i));
        let left_operand = left.map_or(&run.first, |left_link| &left_link.operand);
        let Some(next) = left_operand
            .evaluate_value(&context)
            .and_then(|lhs| lhs.binary(link.op, value, version))
        else {
            break;
        };
        value = next;
        bound = joined_bound(bound, link.op, left_operand, version);
        if is_short_enough(value, bound, version) {
            folded = Some((index, value));
        }
    }
    let Some((start, folded_value)) = folded else {
        return false;
    };
    let first_link = start.checked_sub(1).and_then(|i| run.links.get(i));
    let first_operand = first_link.map_or(&run.first, |link| &link.operand);
    let span = first_operand.span().merge(last.span());
    let Some(literal) = value_to_expr(allocator, folded_value, span) else {
        return false;
    };
    let Some(kept_links) = start.checked_sub(1) else {
        drop(expr);
        *expr = literal;
        return true;
    };
    for link in run.links.get(kept_links..).unwrap_or_default() {
        drop(&link.operand);
    }
    run.links.truncate(start);
    run.links.last_mut().operand = literal;
    true
}

/// Joins the string operands a run of `..` ends with into one.
///
/// `..` groups from the right, so those are the pairs it folds first, and
/// they join in one copy however many there are. A string never joins a
/// number, whose `tostring` is the target's.
fn fold_concat_run<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let string = |operand: &Expression<'ast>| match operand.evaluate_value(&context)? {
        ConstantValue::String(bytes) => Some(bytes),
        ConstantValue::Nil
        | ConstantValue::Boolean(_)
        | ConstantValue::Number(_)
        | ConstantValue::Integer(_) => None,
    };
    // Last first.
    let mut strings: Vec<&[u8]> =
        run.links.iter().rev().map_while(|link| string(&link.operand)).collect();
    let first_link = run.links.len().saturating_sub(strings.len());
    let first_string = if first_link == 0 { string(&run.first) } else { None };
    let is_whole_run = first_string.is_some();
    strings.extend(first_string);
    if strings.len() < 2 {
        return false;
    }
    let mut joined = Vec::with_capacity(strings.iter().map(|bytes| bytes.len()).sum());
    for bytes in strings.iter().rev() {
        joined.extend_from_slice(bytes);
    }
    let first_operand = if is_whole_run {
        &run.first
    } else {
        run.links.get(first_link).map_or(&run.first, |link| &link.operand)
    };
    let span = first_operand.span().merge(run.links.last().operand.span());
    let value = ThinBytes::new(allocator.alloc_slice_copy(&joined));
    let literal = Expression::new_string_literal(value, None, span, allocator);
    if is_whole_run {
        drop(expr);
        *expr = literal;
    } else {
        for link in run.links.get(first_link..).unwrap_or_default() {
            drop(&link.operand);
        }
        run.links.truncate(first_link.saturating_add(1));
        run.links.last_mut().operand = literal;
    }
    true
}

/// Folds a run of `and`s or of `or`s from the left while its first operand's
/// truthiness decides, and drops the operands after one that decides it.
///
/// `false and x` and `true or x` are their first operand, and the rest of
/// the run never runs. `true and x` and `false or x` are `x`, truncated to
/// one value, once the first operand can go. In `x or true or y`, `y` never
/// runs either.
fn fold_logical_run<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::BinaryOp(boxed) = expr else {
        return false;
    };
    let run = &mut **boxed;
    let is_or = run.links.first().op == BinOp::Or;
    let decider =
        run.links.iter().position(|link| link.operand.to_boolean(&context) == Some(is_or));
    if let Some(kept_links) = decider.map(|index| index.saturating_add(1))
        && kept_links < run.links.len()
    {
        for link in run.links.get(kept_links..).unwrap_or_default() {
            drop(&link.operand);
        }
        run.links.truncate(kept_links);
        return true;
    }
    let mut folded_links = 0usize;
    while let Some(is_truthy) = run.first.to_boolean(&context) {
        if is_truthy == is_or {
            for link in run.links.get(folded_links..).unwrap_or_default() {
                drop(&link.operand);
            }
            *expr = run.first.take();
            return true;
        }
        if run.first.may_have_side_effects(&context) {
            break;
        }
        let Some(link) = run.links.get_mut(folded_links) else {
            break;
        };
        drop(&run.first);
        run.first = truncate_multi_value(allocator, link.operand.take());
        folded_links = folded_links.saturating_add(1);
    }
    if folded_links == 0 {
        return false;
    }
    match OneOrMore::from_list(run.links.split_off(folded_links)) {
        Some(rest) => run.links = rest,
        None => *expr = run.first.take(),
    }
    true
}

fn fold_unary<'ast>(
    allocator: &'ast Allocator,
    context: BoundNames,
    expr: &mut Expression<'ast>,
    drop: &mut impl FnMut(&Expression<'ast>),
) -> bool {
    let Expression::UnaryOp(unary) = &*expr else {
        return false;
    };
    // A negated numeral is how a negative number is spelled, so only the
    // numeral's own spelling can get shorter.
    if unary.op == UnOp::Neg && matches!(unary.operand, Expression::Number(_)) {
        return false;
    }
    let version = context.0;
    let span = unary.span;
    let Some(value) = expr
        .evaluate_value(&context)
        .filter(|&value| is_short_enough(value, printed_length_bound(expr, version), version))
    else {
        return false;
    };
    let Some(literal) = value_to_expr(allocator, value, span) else {
        return false;
    };
    drop(expr);
    *expr = literal;
    true
}

/// The bound on a run's length once `op` and `operand` join the operands
/// beside them, whose bound is `bound`.
fn joined_bound(
    bound: Option<usize>,
    op: BinOp,
    operand: &Expression<'_>,
    version: LuaVersion,
) -> Option<usize> {
    let operand_bound = printed_length_bound(operand, version)?;
    Some(bound?.saturating_add(op.static_text().len()).saturating_add(operand_bound))
}

/// The fewest bytes the compact printer writes `expr` in, when it's a number
/// or operators on numbers: their spellings, without the spaces and the
/// parentheses it may add.
fn printed_length_bound(expr: &Expression<'_>, version: LuaVersion) -> Option<usize> {
    match expr {
        Expression::Number(literal) => Some(spell_number(literal.value, version).as_str().len()),
        Expression::UnaryOp(unary) => {
            let operand_bound = printed_length_bound(&unary.operand, version)?;
            Some(unary.op.static_text().len().saturating_add(operand_bound))
        }
        Expression::BinaryOp(run) => {
            run.links.iter().try_fold(printed_length_bound(&run.first, version)?, |bound, link| {
                joined_bound(Some(bound), link.op, &link.operand, version)
            })
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => None,
    }
}

/// Whether the literal for `value` is no longer than the operators it
/// replaces, which take at least `bound` bytes when that's known. Only a
/// number's literal can be longer, and NaN has none.
fn is_short_enough(value: ConstantValue<'_>, bound: Option<usize>, version: LuaVersion) -> bool {
    let ConstantValue::Number(number) = value else {
        return true;
    };
    let Some((is_negative, magnitude)) = number.literal_magnitude() else {
        return false;
    };
    let length =
        usize::from(is_negative).saturating_add(spell_number(magnitude, version).as_str().len());
    bound.is_none_or(|replaced| length <= replaced)
}

/// The expression that spells `value`, or `None` for NaN.
fn value_to_expr<'ast>(
    allocator: &'ast Allocator,
    value: ConstantValue<'ast>,
    span: Span,
) -> Option<Expression<'ast>> {
    Some(match value {
        ConstantValue::Nil => Expression::Nil(span),
        ConstantValue::Boolean(true) => Expression::True(span),
        ConstantValue::Boolean(false) => Expression::False(span),
        ConstantValue::Number(number) => {
            let (is_negative, magnitude) = number.literal_magnitude()?;
            let literal = Expression::new_number(magnitude, None, span, true, allocator);
            if is_negative {
                Expression::new_unary_op(span, UnOp::Neg, literal, allocator)
            } else {
                literal
            }
        }
        // Luau
        ConstantValue::Integer(integer) => Expression::new_integer(integer, None, span, allocator),
        ConstantValue::String(bytes) => {
            Expression::new_string_literal(ThinBytes::new(bytes), None, span, allocator)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const FOLDING: TransformConfig = TransformConfig { fold_constants: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        apply_version(source, LuaVersion::Lua54)
    }

    fn apply_version(source: &str, version: LuaVersion) -> String {
        minify_with(source, version, &FOLDING)
    }

    #[test]
    fn folds_arithmetic_and_concatenation() {
        for (source, expected) in [
            ("local x = 1 + 2", "local x=3"),
            ("local x = 10 * 5", "local x=50"),
            ("local x = \"hello\" .. \" world\"", "local x=\"hello world\""),
            ("local x = 2 + 3 * 4 - 1", "local x=13"),
            ("local x = \"a\" .. \"b\" .. \"c\"", "local x=\"abc\""),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_not_of_a_literal() {
        for (source, expected) in [
            ("local x = not true", "local x=false"),
            ("local x = not false", "local x=true"),
            ("local x = not nil", "local x=true"),
            ("local x = not 5", "local x=false"),
            ("local x = not \"hello\"", "local x=false"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_and_and_or_with_a_literal_left_operand() {
        for (source, expected) in [
            ("local x = true and 42", "local x=42"),
            ("local x = false and 42", "local x=false"),
            ("local x = true or 42", "local x=true"),
            ("local x = false or 42", "local x=42"),
            ("local x = nil and 42", "local x=nil"),
            ("local x = nil or 42", "local x=42"),
            ("return true and f()", "return(f())"),
            ("return 1 and 2 and 3", "return 3"),
            ("return nil and f() or 2", "return 2"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_and_and_or_past_a_pure_left_operand_of_known_truthiness() {
        for (source, expected) in [
            ("local x = ... return {x} and 1", "local x=...return 1"),
            ("local x = ... return x and false or 2", "local x=...return 2"),
            ("local x = ... return x or true or 2", "local x=...return x or true"),
            ("return {g} and 1", "return{g}and 1"),
            ("return g and false or 2", "return g and false or 2"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_what_could_raise_or_call_a_metamethod() {
        for source in [
            "local x=5%0",
            "local x=5//0",
            // `+` may invoke an __add metamethod.
            "return a+0",
            "return#t",
            "return 1|2.5",
        ] {
            assert_eq!(apply(source), source);
        }
    }

    #[test]
    fn keeps_a_number_whose_literal_spells_longer() {
        for source in
            ["return 1/3", "return.1+.2", "return 1/0", "return-1/0", "return 0/0", "return 2^10"]
        {
            assert_eq!(apply(source), source);
        }
        assert_eq!(apply_version("return 2^10", LuaVersion::Lua51), "return 1024");
        assert_eq!(apply("return 1 + 2 + 1/3"), "return 3+1/3");
        assert_eq!(apply("return 1/3 < 1, 0/0 ~= 0/0"), "return true,true");
    }

    #[test]
    fn folds_bitwise_operators_on_integers() {
        for (source, expected) in [
            ("return 1 << 4", "return 16"),
            ("return 6 & 3 | 8", "return 10"),
            ("return ~5", "return-6"),
            ("return 0xff ~ 0xf", "return 240"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_the_length_of_a_string_by_its_value() {
        for (source, expected, version) in [
            ("return #\"hello\"", "return 5", LuaVersion::Lua54),
            ("return #'\\65\\u{e9}' + 1", "return 4", LuaVersion::Lua54),
            ("return #[[\nab]]", "return 2", LuaVersion::Lua51),
            ("return #(\"ab\") * 1.5", "return 3", LuaVersion::Luau),
        ] {
            assert_eq!(apply_version(source, version), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_an_overflow_to_infinity() {
        assert_eq!(apply("local x = 1e308 * 10"), "local x=1e999");
        assert_eq!(apply("local x = -1e308 * 10"), "local x=-1e999");
        assert_eq!(apply("local x = 1e308 * 10 - 1e308 * 10"), "local x=1e999-1e999");
    }

    #[test]
    fn never_inverts_a_negated_comparison() {
        // not (a < b) must not become a >= b: the two call different
        // metamethods, __lt and __le.
        let result = apply("return not (a < b)\n");
        assert!(!result.contains(">="), "{result}");
    }

    #[test]
    fn folds_modulo_with_the_floored_sign() {
        // Lua: -5 % 3 = 1 and 5 % -3 = -1, unlike Rust's truncated modulo.
        assert_eq!(apply("local x = -5 % 3"), "local x=1");
        assert_eq!(apply("local x = 5 % -3"), "local x=-1");
    }

    #[test]
    fn keeps_negative_zero() {
        assert_eq!(apply("local x = 0.0 * -1"), "local x=-0.");
    }

    #[test]
    fn folds_int_min_mixed_with_a_float_only_where_exact() {
        // `0x8000000000000000` wraps to i64::MIN, whose f64 image is exact, so
        // it folds. Its neighbor has no exact image and must not.
        for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
            for (source, expected) in [
                ("return 0x8000000000000000 + 1.5\n", "return-9223372036854776e3"),
                ("return 1.5 + 0x8000000000000000\n", "return-9223372036854776e3"),
                ("return 0x8000000000000000 < 1.5\n", "return true"),
                ("return 0x8000000000000001 + 1.5\n", "return 0x8000000000000001+1.5"),
            ] {
                assert_eq!(apply_version(source, version), expected, "{version:?}");
            }
        }
    }

    #[test]
    fn folds_float_modulo_with_fmod_from_lua53() {
        // 5.3+ luai_nummod is fmod + sign fix: 1e16 % 3 == 1.0 exactly.
        // 5.1's floor formula really does produce 0.0, so mirror it.
        assert_eq!(apply_version("return 1e16 % 3\n", LuaVersion::Lua54), "return 1.");
        assert_eq!(apply_version("return 1e16 % 3\n", LuaVersion::Lua51), "return 0");
    }

    #[test]
    fn folds_float_modulo_keeping_the_sign_of_zero_from_lua53() {
        // fmod(-6.0, 3.0) is -0.0 under 5.3+; the floor formula loses
        // the sign. Observable via 1/x and tostring.
        assert_eq!(apply_version("return -6.0 % 3.0\n", LuaVersion::Lua54), "return-0.");
        assert_eq!(apply_version("return -6.0 % 3.0\n", LuaVersion::Lua51), "return 0");
    }

    #[test]
    fn subtracting_integer_zero_matches_the_lua54_immediate_add() {
        // 5.4+ compiles `x - 0` to OP_ADDI with -0, and -0.0 + 0.0 is +0.0.
        for version in [LuaVersion::Lua54, LuaVersion::Lua55] {
            let result = apply_version("return -0.0 - 0\n", version);
            assert_ne!(result, "return-0.", "{version:?} evaluates this to +0.0");
        }
    }

    #[test]
    fn folds_integer_powers_with_an_exact_result() {
        for (source, expected) in [
            ("return 2 ^ 3\n", "return 8."),
            ("return (-2) ^ 3\n", "return-8."),
            ("return 7 ^ 0\n", "return 1."),
            ("return 2 ^ 53\n", "return 2^53"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_exponentiation_with_an_inexact_result() {
        // An inexact `^` depends on the target's C pow.
        assert_eq!(apply("return 0.3 ^ 3\n"), "return.3^3");
    }

    #[test]
    fn folds_a_run_from_the_side_it_groups_from() {
        for (source, expected) in [
            // Left-associative: the folded prefix stays the run's first.
            ("return 1 + 2 + x + 3", "return 3+x+3"),
            // Right-associative: the folded suffix stays the run's last.
            ("return x .. \"a\" .. \"b\" .. 'c'", "return x..\"abc\""),
            ("return \"a\" .. \"b\" .. x", "return\"a\"..\"b\"..x"),
            ("return \"a\" .. (\"b\") .. \"\\\"\"", "return'ab\"'"),
            ("return 2 ^ 2 ^ 3", "return 256."),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn folds_a_double_negation_through_parentheses() {
        let config = TransformConfig { simplify_parens: true, ..FOLDING };
        for version in [LuaVersion::Lua51, LuaVersion::Luau] {
            assert_eq!(
                minify_with("return -(-0x625)", version, &config),
                "return 1573",
                "{version:?}"
            );
        }
    }

    #[test]
    fn a_negated_numeral_is_already_folded() {
        for source in ["return-1", "return-1.5", "return-0xffffffffffffffff"] {
            assert_eq!(apply(source), source);
        }
        assert_eq!(apply("return - -1"), "return 1");
    }
}
