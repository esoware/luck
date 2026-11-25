use luck_ast::{
    BinaryOp, FunctionBody, GenericFor, IfExpression, IfStatement, NumericFor, RepeatLoop, Type,
    Visit, WhileLoop,
};
use luck_semantic::{AstNode, AstTypesBitset};
use luck_token::BinOp;

use crate::context::{ContextHost, LintContext};
use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::rule::{Rule, RuleOptions, declare_lint_rule};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CyclomaticComplexity {
    max: Option<u32>,
}

// `Category` has no complexity variant, and `Style` fits pressure to refactor best.
declare_lint_rule!(
    CyclomaticComplexity,
    Style,
    Warning,
    "function exceeds the configured cyclomatic complexity threshold",
    options = CyclomaticComplexityOptions
);

/// Options of the `cyclomatic_complexity` rule, which reports nothing until
/// they set `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub(crate) struct CyclomaticComplexityOptions {
    /// Complexity above which a function is flagged.
    max: u32,
}

impl RuleOptions for CyclomaticComplexityOptions {
    fn merge_onto(self, _base: Self) -> Self {
        self
    }
}

impl From<CyclomaticComplexityOptions> for CyclomaticComplexity {
    fn from(options: CyclomaticComplexityOptions) -> Self {
        Self { max: Some(options.max) }
    }
}

impl Rule for CyclomaticComplexity {
    const NODE_TYPES: AstTypesBitset = FunctionNode::NODE_TYPES;

    fn should_run(&self, _host: &ContextHost<'_>) -> bool {
        self.max.is_some()
    }

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'_, 'ast>) {
        let (Some(threshold), Some(function)) = (self.max, FunctionNode::of(node, context.nodes))
        else {
            return;
        };
        let mut points = DecisionPoints::default();
        points.visit_block(&function.body.block);
        // McCabe: one straight-line path plus one per decision point.
        let complexity = points.count + 1;
        if complexity <= threshold {
            return;
        }
        // Slice the name from source so `mod.f` and `a.b:c` appear whole.
        let label = function.name_span.map_or_else(
            || "(anonymous)".to_owned(),
            |name_span| format!("`{}`", String::from_utf8_lossy(name_span.text(context.source))),
        );
        context.diagnostic(
            Finding::new(
                format!(
                    "function {label} has cyclomatic complexity {complexity} (threshold: {threshold})"
                ),
                function.span,
            )
            .with_help("split this function into smaller helpers"),
        );
    }
}

/// One per `if`, `elseif`, and loop, one per `and` or `or`, and one per `if`
/// or `elseif` of an if-expression.
#[derive(Default)]
struct DecisionPoints {
    count: u32,
}

impl DecisionPoints {
    fn count_clauses(&mut self, clause_count: usize) {
        self.count +=
            u32::try_from(clause_count + 1).expect("a source has fewer than `u32::MAX` bytes");
    }
}

impl<'ast> Visit<'ast> for DecisionPoints {
    fn visit_if_stmt(&mut self, if_stmt: &'ast IfStatement<'ast>) {
        self.count_clauses(if_stmt.elseif_clauses.len());
        self.walk_if_stmt(if_stmt);
    }

    fn visit_while_loop(&mut self, while_loop: &'ast WhileLoop<'ast>) {
        self.count += 1;
        self.walk_while_loop(while_loop);
    }

    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop<'ast>) {
        self.count += 1;
        self.walk_repeat_loop(repeat_loop);
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        self.count += 1;
        self.walk_numeric_for(numeric_for);
    }

    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor<'ast>) {
        self.count += 1;
        self.walk_generic_for(generic_for);
    }

    fn visit_binary_op(&mut self, binary_op: &'ast BinaryOp<'ast>) {
        if matches!(binary_op.links.first().op, BinOp::And | BinOp::Or) {
            self.count += u32::try_from(binary_op.links.len())
                .expect("a source has fewer than `u32::MAX` bytes");
        }
        self.walk_binary_op(binary_op);
    }

    fn visit_if_expr(&mut self, if_expr: &'ast IfExpression<'ast>) {
        self.count_clauses(if_expr.elseif_clauses.len());
        self.walk_if_expr(if_expr);
    }

    fn visit_function_body(&mut self, _function_body: &'ast FunctionBody<'ast>) {}

    fn visit_type(&mut self, _ty: &'ast Type<'ast>) {}
}

#[cfg(test)]
mod tests {
    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, Option<&str>)] = &[
        ("function f() if a then end if b then end if c then end if d then end end", None),
        (
            "function f() if a then end if b then end if c then end if d then end end",
            Some(r#"{"max": 100}"#),
        ),
    ];

    const FAIL: &[(&str, Option<&str>)] = &[
        // Four `if` statements = 4 decision points + 1 baseline = 5.
        (
            "function f() if a then end if b then end if c then end if d then end end",
            Some(r#"{"max": 3}"#),
        ),
        ("function my_func() if a then end if b then end end", Some(r#"{"max": 1}"#)),
        (
            "\
local m = {}
function m.sub:run() if a then end end",
            Some(r#"{"max": 1}"#),
        ),
        ("local f = function() if a then end if b then end end", Some(r#"{"max": 1}"#)),
        ("function paren_table_arg() f({a and b}) end", Some(r#"{"max": 0}"#)),
        ("function bare_table_arg() f{a and b} end", Some(r#"{"max": 0}"#)),
        ("function value_and() x = a and b end", Some(r#"{"max": 0}"#)),
        ("function target_or() t[a or b] = 1 end", Some(r#"{"max": 0}"#)),
        ("function return_and() return a and b end", Some(r#"{"max": 0}"#)),
        ("function after_return() do return end return a and b end", Some(r#"{"max": 0}"#)),
        ("function if_and() if a and b then end end", Some(r#"{"max": 0}"#)),
        (
            "function elseif_chain() if a then elseif b then elseif c then end end",
            Some(r#"{"max": 0}"#),
        ),
        ("function while_loop() while a do end end", Some(r#"{"max": 0}"#)),
        ("function repeat_loop() repeat until a end", Some(r#"{"max": 0}"#)),
        ("function numeric_for() for i = 1, 2 do end end", Some(r#"{"max": 0}"#)),
        ("function generic_for() for _ in f do end end", Some(r#"{"max": 0}"#)),
        // `outer` counts neither nested function's `if`, which each counts as
        // its own.
        (
            "\
function outer()
  local f = function() if a then end end
  local function g() if b then end end
end",
            Some(r#"{"max": 0}"#),
        ),
        ("function comp_value() t[1] += a or b end", Some(r#"{"max": 0}"#)),
        ("function comp_target() t[a or b] += 1 end", Some(r#"{"max": 0}"#)),
        ("type function t1(a) if a then return a end end", Some(r#"{"max": 1}"#)),
        // Baseline 1 > 0, so the threshold trips.
        ("function f() end", Some(r#"{"max": 0}"#)),
        // `f` has complexity 3 and `g` 2, so both exceed 1.
        (
            "function f() if a then end if b then end end function g() if c then end end",
            Some(r#"{"max": 1}"#),
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::CyclomaticComplexity, PASS, FAIL).test_and_snapshot();
    }
}
