use luck_ast::{Expression, NumericFor, Statement, Var};
use luck_token::{LuaVersion, UnOp};

use crate::diagnostic::Finding;
use crate::node_type::{NodeType, NodeTypeSet};
use crate::number::signed_number_value;
use crate::rule::{LintContext, NodeRule, impl_rule};

/// Flags numeric `for` ranges that can't iterate or are likely off by one.
///
/// That covers a zero step, a step pointing away from the limit, `0, #t`,
/// `#t, 0` or `#t, 1` with no `-1` step, and a limit the step never lands
/// on. A start above the limit with no step is left to `reversed_for_loop`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ForRange;

impl_rule!(
    ForRange,
    "for_range",
    Correctness,
    Error,
    "numeric `for` range can't iterate or is off by one"
);

impl NodeRule for ForRange {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[NodeType::NumericFor])
    }
    fn on_stmt(&self, stmt: &Statement, context: &LintContext<'_>, findings: &mut Vec<Finding>) {
        if let Statement::NumericFor(numeric_for) = stmt
            && let Some((message, help)) = range_problem(numeric_for, context.semantic.version)
        {
            findings.push(Finding::new(message, numeric_for.span).with_help(help));
        }
    }
}

/// The message and help for what's wrong with a loop's range, if anything.
fn range_problem(numeric_for: &NumericFor, version: LuaVersion) -> Option<(String, &'static str)> {
    let start = signed_number_value(&numeric_for.start, version);
    let limit = signed_number_value(&numeric_for.limit, version);
    let step = numeric_for.step.as_ref().and_then(|step| signed_number_value(step, version));

    // A zero step loops forever before Lua 5.4 and raises an error from 5.4
    // on.
    if step == Some(0.0f64) {
        return Some((
            "numeric `for` loop with step 0 never advances".to_owned(),
            "use a nonzero step or rewrite as `while`",
        ));
    }

    if let (Some(start_value), Some(limit_value), Some(step_value)) = (start, limit, step) {
        if step_value < 0.0f64 && start_value < limit_value {
            return Some((
                format!(
                    "numeric `for` from {start_value} to {limit_value} with a negative step never iterates"
                ),
                "swap start and limit, or use a positive step",
            ));
        }
        // `reversed_for_loop` covers a start above the limit with no step.
        if step_value > 0.0f64 && start_value > limit_value {
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
    if start == Some(0.0f64) && is_length_of_identifier(&numeric_for.limit) {
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
/// `reversed_for_loop` needs a literal start, so it never fires here.
fn length_start_problem(
    numeric_for: &NumericFor,
    limit_value: f64,
    step: Option<f64>,
) -> Option<(String, &'static str)> {
    let has_step_expr = numeric_for.step.is_some();
    match limit_value {
        0.0f64 if !has_step_expr || step.is_some_and(|step_value| step_value > 0.0f64) => Some((
            "numeric `for` from a length to 0 needs a `-1` step, and Lua sequences are 1-indexed"
                .to_owned(),
            "use `for i = #t, 1, -1 do` to iterate the sequence backwards",
        )),
        1.0f64 if !has_step_expr => Some((
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
#[expect(
    clippy::float_arithmetic,
    reason = "this rule reproduces the Lua VM arithmetic in doubles"
)]
fn limit_landing_problem(
    numeric_for: &NumericFor,
    start_value: f64,
    limit_value: f64,
    step: Option<f64>,
) -> Option<(String, &'static str)> {
    let step_value = match &numeric_for.step {
        None => 1.0f64,
        Some(_) => step?,
    };
    if !start_value.is_finite()
        || !limit_value.is_finite()
        || !step_value.is_finite()
        || step_value == 0.0f64
    {
        return None;
    }
    let count = ((limit_value - start_value) / step_value).floor();
    #[expect(
        clippy::suboptimal_flops,
        reason = "the VM rounds the product and the sum separately; a fused multiply-add would not"
    )]
    let last_value = start_value + count * step_value;
    let mismatch = limit_value - last_value;
    (count >= 0.0f64 && mismatch.is_finite() && mismatch.fract() != 0.0f64).then(|| {
        (
            format!(
                "numeric `for` loop ends at {last_value} instead of {limit_value}, did you forget to specify a step?"
            ),
            "pick a step that lands on the limit, or adjust the limit",
        )
    })
}

/// Whether `expr` is `#name`.
fn is_length_of_identifier(expr: &Expression) -> bool {
    if let Expression::UnaryOp(unary) = expr
        && unary.op == UnOp::Len
        && let Expression::Var(var) = &unary.operand
    {
        return matches!(var, Var::Name(_));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&ForRange, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_zero_step() {
        let diags = run("for i = 1, 10, 0 do end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("step 0"));
    }

    #[test]
    fn flags_negative_step_ascending() {
        let diags = run("for i = 1, 10, -1 do end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("negative step"));
    }

    #[test]
    fn flags_zero_start_with_length() {
        let diags = run("for i = 0, #t do end");
        assert_eq!(diags.len(), 1, "expected 1 diag, got: {diags:?}");
        assert!(diags[0].message.contains("1-indexed"));
    }

    #[test]
    fn flags_descending_with_positive_step() {
        let diags = run("for i = 10, 1, 1 do end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("positive step"));
    }

    #[test]
    fn flags_hex_bounds() {
        let diags = run("for i = 0x10, 1, 1 do end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("from 16 to 1"), "{diags:?}");
    }

    #[test]
    fn ignores_ascending_range_without_step() {
        let diags = run("for i = 1, 10 do end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_descending_range_with_negative_step() {
        let diags = run("for i = 10, 1, -1 do end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_zero_start_with_plain_number_limit() {
        // `0, 5` may be deliberate. Only `0, #t` fires.
        let diags = run("for i = 0, 5 do end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_length_to_zero() {
        let diags = run("local t = {}\nfor i = #t, 0 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("1-indexed"), "{diags:?}");
    }

    #[test]
    fn flags_length_to_one_without_step() {
        let diags = run("local t = {}\nfor i = #t, 1 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("backwards"), "{diags:?}");
    }

    #[test]
    fn flags_fractional_limit_mismatch() {
        let diags = run("for i = 1, 8.75 do print(i) end");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("ends at 8"), "{diags:?}");
    }

    #[test]
    fn ignores_length_to_one_with_negative_step() {
        let diags = run("local t = {}\nfor i = #t, 1, -1 do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_integer_stride_missing_limit() {
        // `for i = 1, 10, 2` stops at 9; integer strides are idiomatic.
        let diags = run("for i = 1, 10, 2 do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_fractional_step_landing_on_limit() {
        let diags = run("for i = 1, 2.5, 0.5 do print(i) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_reversed_for_loop_no_step() {
        // With no step, this is `reversed_for_loop`'s case.
        let diags = run("for i = 10, 1 do end");
        assert!(diags.is_empty(), "for_range must defer to reversed_for_loop: {diags:?}");
    }

    #[test]
    fn flags_underscore_separated_bound_on_luau() {
        let diags = run_rule(&ForRange, "for i = 1_000, 1, 1 do print(i) end", LuaVersion::Luau);
        assert_eq!(diags.len(), 1, "1_000 is a number literal above the limit: {diags:?}");
    }
}
