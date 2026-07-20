use luck_ast::Expression;
use luck_ast::node::{AstTypesBitset, NodeType};
use luck_ast::stmt::NumericFor;
use luck_token::UnOp;

use crate::diagnostic::*;
use crate::rule::{LintContext, NodeRule, Rule};

/// Numeric-for ranges that cannot iterate or are obvious off-by-one
/// mistakes (zero step, negative step with start < end, start > end with
/// no negative step, 0-based iteration of a 1-based sequence length).
///
/// Relationship to `reversed_for_loop`: the older rule fires only when
/// start > end without any step. This rule fires in the strictly larger
/// set of impossible-iteration cases - including the "0, #t" off-by-one.
/// The "start > end, no step" case is left to `reversed_for_loop` so the
/// two rules do not double-fire on the same source. Other shapes (zero
/// step, negative step with start < end, 0, #t) are unique to this rule.
pub struct ForRange;

impl Rule for ForRange {
    fn name(&self) -> &'static str {
        "for_range"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "numeric for range cannot iterate or is off-by-one"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

struct RangeChecker<'a> {
    out: &'a mut Vec<LintDiagnostic>,
}

/// Parse a numeric literal, optionally wrapped in a unary minus.
fn literal_number(expr: &Expression) -> Option<f64> {
    match expr {
        Expression::Number(literal) => literal.text.parse().ok(),
        Expression::UnaryOp(unop) if unop.op == UnOp::Neg => {
            literal_number(&unop.operand).map(|value| -value)
        }
        _ => None,
    }
}

/// Recognize `#identifier` - the length of an obviously-positional table
/// or string. Used for the 0,#t off-by-one heuristic.
fn is_length_of_identifier(expr: &Expression) -> bool {
    if let Expression::UnaryOp(unop) = expr
        && unop.op == UnOp::Len
        && let Expression::Var(var) = &unop.operand
    {
        return matches!(var, luck_ast::expr::Var::Name(_));
    }
    false
}

impl RangeChecker<'_> {
    fn check_for(&mut self, num_for: &NumericFor) {
        let start = literal_number(&num_for.start);
        let limit = literal_number(&num_for.limit);
        let step = num_for.step.as_ref().and_then(literal_number);

        // Zero step is an infinite loop on Lua 5.1-5.3, a runtime error
        // on 5.4+. Either way the author meant something else.
        if let Some(s) = step
            && s == 0.0
        {
            self.out.push(
                LintDiagnostic::new(
                    "for_range",
                    "numeric for loop with step 0 never advances".to_string(),
                    num_for.span,
                )
                .with_help("use a nonzero step or rewrite as `while`".to_string()),
            );
            return;
        }

        // Negative step with start < end never iterates.
        if let (Some(start_value), Some(limit_value), Some(step_value)) = (start, limit, step)
            && step_value < 0.0
            && start_value < limit_value
        {
            self.out.push(LintDiagnostic::new("for_range", format!(
                    "numeric for from {start_value} to {limit_value} with negative step never iterates"
                ), num_for.span).with_help("swap start and limit, or use a positive step".to_string()));
            return;
        }

        // Start > limit with no explicit negative step never iterates.
        // `reversed_for_loop` already covers the no-step case; we only
        // fire here when the step is present and non-negative.
        if let (Some(start_value), Some(limit_value)) = (start, limit)
            && start_value > limit_value
            && let Some(step_value) = step
            && step_value > 0.0
        {
            self.out.push(LintDiagnostic::new("for_range", format!(
                    "numeric for from {start_value} to {limit_value} with positive step never iterates"
                ), num_for.span).with_help("swap start and limit, or use a negative step".to_string()));
            return;
        }

        // 0,#t - Lua sequences are 1-indexed, iterating from 0 reads an
        // out-of-band entry and skips `t[#t]`. Only fire when the limit
        // is a `#name` expression, since `0, n` may be intentional.
        if let Some(start_value) = start
            && start_value == 0.0
            && is_length_of_identifier(&num_for.limit)
        {
            self.out.push(
                LintDiagnostic::new(
                    "for_range",
                    "numeric for starts at 0; Lua sequences are 1-indexed".to_string(),
                    num_for.span,
                )
                .with_help("use `for i = 1, #t do` to iterate the whole sequence".to_string()),
            );
            return;
        }

        // #t,0 and #t,1 - iterating a sequence from its length needs a -1
        // step, and a 0 limit also misses that sequences are 1-indexed.
        // reversed_for_loop needs a literal start, so it never fires here.
        if is_length_of_identifier(&num_for.start)
            && let Some(limit_value) = limit
        {
            let has_step_expr = num_for.step.is_some();
            if limit_value == 0.0 && (!has_step_expr || step.is_some_and(|s| s > 0.0)) {
                self.out.push(
                    LintDiagnostic::new(
                        "for_range",
                        "numeric for from a length to 0 should iterate backwards but has no -1 step, and 0 should probably be 1 since Lua sequences are 1-indexed".to_string(),
                        num_for.span,
                    )
                    .with_help(
                        "use `for i = #t, 1, -1 do` to iterate the sequence backwards".to_string(),
                    ),
                );
                return;
            }
            if limit_value == 1.0 && !has_step_expr {
                self.out.push(
                    LintDiagnostic::new(
                        "for_range",
                        "numeric for from a length to 1 should iterate backwards; did you forget a -1 step?".to_string(),
                        num_for.span,
                    )
                    .with_help("add the step: `for i = #t, 1, -1 do`".to_string()),
                );
                return;
            }
        }

        // A limit the step never lands on makes the loop stop short:
        // `for i = 1, 8.75` ends at 8. Only fractional mismatches fire -
        // integer strides like `for i = 1, 10, 2` are idiomatic.
        let effective_step = match &num_for.step {
            None => Some(1.0),
            Some(_) => step,
        };
        if let (Some(start_value), Some(limit_value), Some(step_value)) =
            (start, limit, effective_step)
            && start_value.is_finite()
            && limit_value.is_finite()
            && step_value.is_finite()
            && step_value != 0.0
        {
            let count = ((limit_value - start_value) / step_value).floor();
            let last_value = start_value + count * step_value;
            let mismatch = limit_value - last_value;
            if count >= 0.0 && last_value != limit_value && mismatch != mismatch.floor() {
                self.out.push(
                    LintDiagnostic::new(
                        "for_range",
                        format!(
                            "numeric for loop ends at {last_value} instead of {limit_value}; did you forget to specify a step?"
                        ),
                        num_for.span,
                    )
                    .with_help(
                        "pick a step that lands on the limit, or adjust the limit".to_string(),
                    ),
                );
            }
        }
    }
}

impl NodeRule for ForRange {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[NodeType::NumericFor]);
        Some(&TYPES)
    }
    fn on_statement(
        &self,
        stmt: &luck_ast::Statement,
        _ctx: &LintContext,
        out: &mut Vec<LintDiagnostic>,
    ) {
        if let luck_ast::Statement::NumericFor(num_for) = stmt {
            RangeChecker { out }.check_for(num_for);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&ForRange, source, LuaVersion::Lua54)
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
    fn ignores_happy_path() {
        let diags = run("for i = 1, 10 do end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_happy_descending() {
        let diags = run("for i = 10, 1, -1 do end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_zero_start_with_plain_number_limit() {
        // 0,5 may be deliberate; only flag `0, #t` shape.
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
        // `for i = 10, 1 do` (no step) is the reversed_for_loop case.
        // for_range should NOT fire here.
        let diags = run("for i = 10, 1 do end");
        assert!(
            diags.is_empty(),
            "for_range must defer to reversed_for_loop: {diags:?}"
        );
    }
}
