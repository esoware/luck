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
        let complexity = decision_points(&function.body.block).saturating_add(1);
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
    use luck_token::LuaVersion;

    use crate::config::LintConfig;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule_with_config;
    use crate::rules::RuleId;

    fn config(threshold: Option<u32>) -> LintConfig {
        let setting = threshold.map_or_else(
            || r#""warn""#.to_owned(),
            |max| format!(r#"["warn", {{ "max": {max} }}]"#),
        );
        serde_json::from_str(&format!(r#"{{ "rules": {{ "cyclomatic_complexity": {setting} }} }}"#))
            .expect("a valid `lint` section")
    }

    fn run(source: &str, threshold: Option<u32>) -> Vec<Message> {
        run_rule_with_config(
            RuleId::CyclomaticComplexity,
            source,
            LuaVersion::Lua54,
            &config(threshold),
        )
    }

    #[test]
    fn flags_when_complexity_exceeds_threshold() {
        // Four `if` statements = 4 decision points + 1 baseline = 5.
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, Some(3));
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("complexity 5"), "msg: {}", diags[0].message);
    }

    #[test]
    fn ignores_when_threshold_is_none() {
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, None);
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_when_threshold_is_high() {
        let source = "function f() if a then end if b then end if c then end if d then end end";
        let diags = run(source, Some(100));
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_named_function_by_name() {
        let source = "function my_func() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags[0].message,
            "function `my_func` has cyclomatic complexity 3 (threshold: 1)"
        );
    }

    #[test]
    fn flags_method_by_its_full_name() {
        let source = "\
local m = {}
function m.sub:run() if a then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("`m.sub:run`"), "{diags:?}");
    }

    #[test]
    fn flags_anonymous_function_as_anonymous() {
        let source = "local f = function() if a then end if b then end end";
        let diags = run(source, Some(1));
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert!(diags[0].message.contains("(anonymous)"), "msg: {}", diags[0].message);
    }

    #[test]
    fn flags_decision_points_in_table_call_arguments() {
        for source in [
            "function paren_table_arg() f({a and b}) end",
            "function bare_table_arg() f{a and b} end",
        ] {
            let diags = run(source, Some(0));
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_decision_points_in_assignment_targets() {
        for source in
            ["function value_and() x = a and b end", "function target_or() t[a or b] = 1 end"]
        {
            let diags = run(source, Some(0));
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_decision_points_in_return_exprs() {
        let diags = run("function return_and() return a and b end", Some(0));
        assert!(diags[0].message.contains("complexity 2"), "{diags:?}");
    }

    #[test]
    fn flags_decision_points_in_compound_assignment_targets() {
        let config = config(Some(0));
        for source in [
            "function comp_value() t[1] += a or b end",
            "function comp_target() t[a or b] += 1 end",
        ] {
            let diags = run_rule_with_config(
                RuleId::CyclomaticComplexity,
                source,
                LuaVersion::Luau,
                &config,
            );
            assert!(diags[0].message.contains("complexity 2"), "{source}: {diags:?}");
        }
    }

    #[test]
    fn flags_type_function_bodies() {
        let config = config(Some(1));
        let source = "type function t1(a) if a then return a end end";
        let diags =
            run_rule_with_config(RuleId::CyclomaticComplexity, source, LuaVersion::Luau, &config);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].message, "function `t1` has cyclomatic complexity 2 (threshold: 1)");
    }

    #[test]
    fn flags_empty_function_over_zero_threshold() {
        let source = "function f() end";
        let diags = run(source, Some(0));
        // Baseline 1 > 0, so the threshold trips.
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_each_function_independently() {
        let source = "function f() if a then end if b then end end function g() if c then end end";
        let diags = run(source, Some(1));
        // `f` has complexity 3 (>1); `g` has complexity 2 (>1). Both fire.
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }
}
