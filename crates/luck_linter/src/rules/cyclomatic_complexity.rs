use luck_semantic::{AstNode, AstTypesBitset};

use crate::cfg::decision_points;
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

    fn run<'ast>(&self, node: &AstNode<'ast>, context: &LintContext<'ast>) {
        let (Some(threshold), Some(function)) = (self.max, FunctionNode::of(node, context.nodes))
        else {
            return;
        };
        // McCabe: one straight-line path plus one per decision point.
        let complexity = decision_points(&function.body.block) + 1;
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

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use crate::fixtures::Tester;
    use crate::rules::RuleId;

    const PASS: &[(&str, LuaTarget, Option<&str>)] = &[
        (
            "function f() if a then end if b then end if c then end if d then end end",
            LuaTarget::Lua54,
            None,
        ),
        (
            "function f() if a then end if b then end if c then end if d then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 100}"#),
        ),
    ];

    const FAIL: &[(&str, LuaTarget, Option<&str>)] = &[
        // Four `if` statements = 4 decision points + 1 baseline = 5.
        (
            "function f() if a then end if b then end if c then end if d then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 3}"#),
        ),
        (
            "function my_func() if a then end if b then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 1}"#),
        ),
        (
            "\
local m = {}
function m.sub:run() if a then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 1}"#),
        ),
        (
            "local f = function() if a then end if b then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 1}"#),
        ),
        ("function paren_table_arg() f({a and b}) end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        ("function bare_table_arg() f{a and b} end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        ("function value_and() x = a and b end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        ("function target_or() t[a or b] = 1 end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        ("function return_and() return a and b end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        ("function comp_value() t[1] += a or b end", LuaTarget::Luau, Some(r#"{"max": 0}"#)),
        ("function comp_target() t[a or b] += 1 end", LuaTarget::Luau, Some(r#"{"max": 0}"#)),
        ("type function t1(a) if a then return a end end", LuaTarget::Luau, Some(r#"{"max": 1}"#)),
        // Baseline 1 > 0, so the threshold trips.
        ("function f() end", LuaTarget::Lua54, Some(r#"{"max": 0}"#)),
        // `f` has complexity 3 and `g` 2, so both exceed 1.
        (
            "function f() if a then end if b then end end function g() if c then end end",
            LuaTarget::Lua54,
            Some(r#"{"max": 1}"#),
        ),
    ];

    #[test]
    fn test() {
        Tester::new(RuleId::CyclomaticComplexity, PASS, FAIL).test_and_snapshot();
    }
}
