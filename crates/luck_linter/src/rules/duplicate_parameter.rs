use luck_ast::{Expression, Statement};

use crate::diagnostic::{Category, Finding, Severity};
use crate::function_node::FunctionNode;
use crate::node_type::NodeTypeSet;
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct DuplicateParameter;

impl Rule for DuplicateParameter {
    fn name(&self) -> &'static str {
        "duplicate_parameter"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "function declares two parameters with the same name"
    }
}

impl NodeRule for DuplicateParameter {
    fn node_types(&self) -> NodeTypeSet {
        FunctionNode::NODE_TYPES
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Some(function) = FunctionNode::of_statement(stmt) {
            findings.extend(duplicate_params(function));
        }
    }
    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        findings: &mut Vec<Finding>,
    ) {
        if let Some(function) = FunctionNode::of_expression(expr) {
            findings.extend(duplicate_params(function));
        }
    }
}

/// Flags each parameter named like an earlier one, or `self` in a colon
/// method, which already has an implicit `self`. `_` may repeat.
fn duplicate_params(function: FunctionNode<'_>) -> impl Iterator<Item = Finding> {
    let params = function.body.params.items.as_slice();
    params.iter().enumerate().filter_map(move |(index, param)| {
        let name = param.name.identifier().filter(|&name| name != "_")?;
        let is_duplicate = (function.is_method && name == "self")
            || params.iter().take(index).any(|earlier| earlier.name.identifier() == Some(name));
        is_duplicate.then(|| {
            Finding::new(format!("parameter `{name}` is already defined"), param.name.span)
        })
    })
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, offset_to_index};

    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&DuplicateParameter, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_parameter() {
        let source = "local function f(a, b, a) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`a`"));
        // The second occurrence is the one flagged.
        assert_eq!(offset_to_index(diags[0].span.start), source.rfind('a').unwrap());
    }

    #[test]
    fn flags_duplicate_in_function_expression() {
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
        let diags = run("local t = {}\nfunction t:m(self) end");
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
        let diags = run("local t = {}\nfunction t.m(self) end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
