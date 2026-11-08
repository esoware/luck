use luck_ast::{Expression, FunctionBody, NodeType, NodeTypeSet, Statement};
use luck_token::TokenKind;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
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

fn check_params(body: &FunctionBody, is_method: bool, diags: &mut Vec<LintDiagnostic>) {
    let mut seen_names: Vec<&str> = Vec::new();
    // A colon method already has an implicit `self`.
    if is_method {
        seen_names.push("self");
    }
    for param in body.params.iter() {
        let TokenKind::Identifier(name) = &param.name.kind else {
            continue;
        };
        if name == "_" {
            continue;
        }
        if seen_names.iter().any(|previous| *previous == name.as_str()) {
            diags.push(LintDiagnostic::new(
                "duplicate_parameter",
                format!("parameter `{name}` is already defined"),
                param.name.span,
            ));
        } else {
            seen_names.push(name.as_str());
        }
    }
}

impl NodeRule for DuplicateParameter {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::FunctionDecl,
            NodeType::LocalFunction,
            NodeType::GlobalFunction,
            NodeType::FunctionDef,
        ])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        match stmt {
            Statement::FunctionDecl(decl) => {
                check_params(&decl.body, decl.name.method.is_some(), diags);
            }
            Statement::LocalFunction(local_function) => {
                check_params(&local_function.body, false, diags);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                check_params(&global_function.body, false, diags);
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => {}
        }
    }

    fn on_expression(
        &self,
        expr: &Expression,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        if let Expression::FunctionDef(function) = expr {
            check_params(&function.body, false, diags);
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaVersion, offset_to_index};

    use super::*;
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
