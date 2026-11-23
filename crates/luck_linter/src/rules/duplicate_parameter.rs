use luck_ast::{Expression, Statement};
use luck_token::MAX_ACTIVE_LOCALS;

use crate::diagnostic::Finding;
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, impl_rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateParameter;

impl_rule!(
    DuplicateParameter,
    "duplicate_parameter",
    Correctness,
    Error,
    "function declares two parameters with the same name"
);

impl NodeRule for DuplicateParameter {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_stmt(
        &self,
        stmt: &Statement<'_>,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Some(function) = FunctionNode::of_stmt(stmt) {
            check_params(function, findings);
        }
    }
    fn on_expr(
        &self,
        expr: &Expression<'_>,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Some(function) = FunctionNode::of_expr(expr) {
            check_params(function, findings);
        }
    }
}

/// Flags each parameter named like an earlier one, or `self` in a colon
/// method, which already has an implicit `self`. `_` may repeat, and a Lua
/// 5.5 named vararg counts as the last parameter.
///
/// Parameters are locals, so no Lua compiles a function with more than
/// [`MAX_ACTIVE_LOCALS`] of them, and one that has more isn't checked.
fn check_params(function: FunctionNode<'_>, findings: &mut Vec<Finding>) {
    let params = &function.body.params;
    let vararg_name = function.body.vararg.as_deref().and_then(|vararg| vararg.name.as_ref());
    if params.len().saturating_add(usize::from(vararg_name.is_some())) > MAX_ACTIVE_LOCALS {
        return;
    }
    for (i, param_name) in params.iter().map(|param| &param.name).chain(vararg_name).enumerate() {
        let name = param_name.text;
        if name == "_" {
            continue;
        }
        let first = params.iter().take(i).find(|earlier| earlier.name.text == name);
        if first.is_some() || (function.is_method && name == "self") {
            let finding =
                Finding::new(format!("parameter `{name}` is already defined"), param_name.span);
            findings.push(match first {
                Some(earlier) => finding.with_label(earlier.name.span, "first defined here"),
                None => finding.with_help("a method declared with `:` already has `self`"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, offset_to_index};

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&DuplicateParameter, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_parameter() {
        let source = "local function f(a, b, a) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`a`"));
        // The second occurrence is the one flagged.
        assert_eq!(offset_to_index(diags[0].span().start), source.rfind('a').unwrap());
    }

    #[test]
    fn flags_duplicate_in_function_expr() {
        let diags = run("local f = function(x, x) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn flags_duplicate_in_function_decl() {
        let diags = run("function f(a, a) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn flags_every_duplicate_occurrence() {
        let diags = run("local function f(a, a, a) end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_explicit_self_in_colon_method() {
        let diags = run("\
local t = {}
function t:m(self) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`self`"));
    }

    #[test]
    fn flags_duplicate_in_global_function() {
        let diags = run_rule(&DuplicateParameter, "global function f(a, a) end", LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_distinct_parameters() {
        let diags = run("local function f(a, b, c) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_parameters() {
        let diags = run("local function f(_, _) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_self_in_dot_function() {
        let diags = run("\
local t = {}
function t.m(self) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_named_vararg_matching_parameter() {
        let source = "local function f(a, ...a) return a end";
        let diags = run_rule(&DuplicateParameter, source, LuaVersion::Lua55);
        assert_eq!(diags.len(), 1, "the named vararg repeats parameter `a`: {diags:?}");
    }

    #[test]
    fn flags_each_repeat_in_a_list_at_the_local_limit() {
        let params = (0..100u32).map(|i| format!("p{i}")).collect::<Vec<_>>().join(", ");
        let diags = run(&format!("local function f({params}, {params}) end"));
        assert_eq!(diags.len(), 100, "{diags:?}");
    }

    #[test]
    fn ignores_a_list_past_the_local_limit() {
        let params = (0..101u32).map(|i| format!("p{i}")).collect::<Vec<_>>().join(", ");
        let diags = run(&format!("local function f({params}, {params}) end"));
        assert!(diags.is_empty(), "{diags:?}");
    }
}
