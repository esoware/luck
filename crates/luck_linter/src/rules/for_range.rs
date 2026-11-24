use luck_ast::{AstKind, AstType, Expression, NumericFor};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::UnOp;
use luck_value::ValueContext;

use crate::context::LintContext;
use crate::diagnostic::Finding;
use crate::number::number_value;
use crate::rule::{Rule, declare_lint_rule};

/// Flags numeric `for` ranges that can't iterate or are likely off by one.
///
/// That covers a zero step, a step pointing away from the limit, `0, #t`,
/// `#t, 0` or `#t, 1` with no `-1` step, and a limit the step never lands
/// on. A start above the limit with no step is left to `reversed_for_loop`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ForRange;

declare_lint_rule!(
    ForRange,
    Correctness,
    Error,
    "numeric `for` range can't iterate or is off by one"
);

impl Rule for ForRange {
    const NODE_TYPES: AstTypesBitset = AstTypesBitset::from_types(&[AstType::NumericFor]);

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        if let AstKind::NumericFor(numeric_for) = node.kind()
            && let Some((message, help)) = range_problem(numeric_for, context)
        {
            context.diagnostic(Finding::new(message, numeric_for.span).with_help(help));
        }
    }
}

fn range_problem(
    numeric_for: &NumericFor<'_>,
    context: &dyn ValueContext,
) -> Option<(String, &'static str)> {
    let start = number_value(&numeric_for.start, context);
    let limit = number_value(&numeric_for.limit, context);
    let step = numeric_for.step.as_ref().and_then(|step| number_value(step, context));

    // A zero step loops forever before Lua 5.4 and raises an error from 5.4
    // on.
    if step == Some(0.0) {
        return Some((
            "numeric `for` loop with step 0 never advances".to_owned(),
            "use a nonzero step or rewrite as `while`",
        ));
    }

    if let (Some(start_value), Some(limit_value), Some(step_value)) = (start, limit, step) {
        if step_value < 0.0 && start_value < limit_value {
            return Some((
                format!(
                    "numeric `for` from {start_value} to {limit_value} with a negative step never iterates"
                ),
                "swap start and limit, or use a positive step",
            ));
        }
        // `reversed_for_loop` covers a start above the limit with no step.
        if step_value > 0.0 && start_value > limit_value {
            return Some((
                format!(
                    "numeric `for` from {start_value} to {limit_value} with a positive step never iterates"
                ),
                "swap start and limit, or use a negative step",
            ));
        }
    }

    // Sequences start at 1, so `0, #t` also visits `t[0]`. Only a `#name`
    // limit fires, since `0, n` may be intentional.
    if start == Some(0.0) && is_length_of_identifier(&numeric_for.limit) {
        return Some((
            "numeric `for` starts at 0, but Lua sequences are 1-indexed".to_owned(),
            "use `for i = 1, #t do` to iterate the whole sequence",
        ));
    }

    if is_length_of_identifier(&numeric_for.start) {
        return length_start_problem(numeric_for, limit?, step);
    }

    limit_landing_problem(numeric_for, start?, limit?, step)
}

/// Flags `#t, 0` and `#t, 1` with no `-1` step. Counting down from a length
/// needs one, and a 0 limit also visits `t[0]`.
///
/// `reversed_for_loop` needs a start that evaluates to a number, so it never
/// fires here.
fn length_start_problem(
    numeric_for: &NumericFor<'_>,
    limit_value: f64,
    step: Option<f64>,
) -> Option<(String, &'static str)> {
    let has_step_expr = numeric_for.step.is_some();
    match limit_value {
        0.0 if !has_step_expr || step.is_some_and(|step_value| step_value > 0.0) => Some((
            "numeric `for` from a length to 0 needs a `-1` step, and Lua sequences are 1-indexed"
                .to_owned(),
            "use `for i = #t, 1, -1 do` to iterate the sequence backwards",
        )),
        1.0 if !has_step_expr => Some((
            "numeric `for` from a length to 1 should iterate backwards, did you forget a `-1` step?"
                .to_owned(),
            "add the step: `for i = #t, 1, -1 do`",
        )),
        _ => None,
    }
}

/// Flags a limit the step never lands on, as in `for i = 1, 8.75`, which
/// ends at 8.
///
/// Only fractional mismatches fire, since integer strides like
/// `for i = 1, 10, 2` are idiomatic.
fn limit_landing_problem(
    numeric_for: &NumericFor<'_>,
    start_value: f64,
    limit_value: f64,
    step: Option<f64>,
) -> Option<(String, &'static str)> {
    let step_value = match &numeric_for.step {
        None => 1.0,
        Some(_) => step?,
    };
    if !start_value.is_finite()
        || !limit_value.is_finite()
        || !step_value.is_finite()
        || step_value == 0.0
    {
        return None;
    }
    let count = ((limit_value - start_value) / step_value).floor();
    let last_value = start_value + count * step_value;
    let mismatch = limit_value - last_value;
    (count >= 0.0 && mismatch.is_finite() && mismatch.fract() != 0.0).then(|| {
        (
            format!(
                "numeric `for` loop ends at {last_value} instead of {limit_value}, did you forget to specify a step?"
            ),
            "pick a step that lands on the limit, or adjust the limit",
        )
    })
}

fn is_length_of_identifier(expr: &Expression<'_>) -> bool {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Len
    {
        return matches!(unary.operand, Expression::Name(_));
    }
    false
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget)] = &[
        ("for i = 1, 10 do end", LuaTarget::Lua54),
        ("for i = 10, 1, -1 do end", LuaTarget::Lua54),
        // `0, 5` may be deliberate. Only `0, #t` fires.
        ("for i = 0, 5 do end", LuaTarget::Lua54),
        (
            "\
local t = {}
for i = #t, 1, -1 do print(i) end",
            LuaTarget::Lua54,
        ),
        // `for i = 1, 10, 2` stops at 9; integer strides are idiomatic.
        ("for i = 1, 10, 2 do print(i) end", LuaTarget::Lua54),
        ("for i = 1, 2.5, 0.5 do print(i) end", LuaTarget::Lua54),
        // With no step, this is `reversed_for_loop`'s case.
        ("for i = 10, 1 do end", LuaTarget::Lua54),
    ];

    const FAIL: &[(&str, LuaTarget)] = &[
        ("for i = 1, 10, 0 do end", LuaTarget::Lua54),
        ("for i = 1, 10, -1 do end", LuaTarget::Lua54),
        ("for i = 0, #t do end", LuaTarget::Lua54),
        ("for i = 10, 1, 1 do end", LuaTarget::Lua54),
        ("for i = 0x10, 1, 1 do end", LuaTarget::Lua54),
        (
            "\
local t = {}
for i = #t, 0 do print(i) end",
            LuaTarget::Lua54,
        ),
        (
            "\
local t = {}
for i = #t, 1 do print(i) end",
            LuaTarget::Lua54,
        ),
        ("for i = 1, 8.75 do print(i) end", LuaTarget::Lua54),
        // `1_000` is a number literal above the limit.
        ("for i = 1_000, 1, 1 do print(i) end", LuaTarget::Luau),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::ForRange, PASS, FAIL).test_and_snapshot();
    }
}
