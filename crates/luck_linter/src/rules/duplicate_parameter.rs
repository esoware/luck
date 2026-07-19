use luck_ast::Statement;
use luck_ast::expr::Expression;
use luck_ast::shared::FunctionBody;
use luck_token::TokenKind;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};
use luck_ast::node::{AstTypesBitset, NodeType};

pub struct DuplicateParameter;

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
        "Function declares two parameters with the same name."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        crate::bus::run_single(self, ctx)
    }
}

fn check_params(body: &FunctionBody, is_method: bool, out: &mut Vec<LintDiagnostic>) {
    let mut seen: Vec<&str> = Vec::new();
    // Colon methods declare an implicit `self`, so an explicit `self`
    // parameter is already a duplicate.
    if is_method {
        seen.push("self");
    }
    for param in body.params.iter() {
        let TokenKind::Identifier(name) = &param.name.kind else {
            continue;
        };
        if name == "_" {
            continue;
        }
        if seen.iter().any(|prev| *prev == name.as_str()) {
            out.push(LintDiagnostic::new(
                "duplicate_parameter",
                format!("parameter '{name}' is already defined"),
                param.name.span,
            ));
        } else {
            seen.push(name.as_str());
        }
    }
}

impl NodeRule for DuplicateParameter {
    fn node_types(&self) -> Option<&'static AstTypesBitset> {
        static TYPES: AstTypesBitset = AstTypesBitset::from_types(&[
            NodeType::FunctionDecl,
            NodeType::LocalFunction,
            NodeType::GlobalFunction,
            NodeType::FunctionDef,
        ]);
        Some(&TYPES)
    }
    fn on_statement(&self, stmt: &Statement, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        match stmt {
            Statement::FunctionDecl(decl) => {
                check_params(&decl.body, decl.name.method.is_some(), out);
            }
            Statement::LocalFunction(func) => check_params(&func.body, false, out),
            Statement::GlobalFunction(func) => check_params(&func.body, false, out), // Lua 5.5
            _ => {}
        }
    }

    fn on_expression(&self, expr: &Expression, _ctx: &LintContext, out: &mut Vec<LintDiagnostic>) {
        if let Expression::FunctionDef(def) = expr {
            check_params(&def.body, false, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&DuplicateParameter, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_parameter() {
        let source = "local function f(a, b, a) end";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'a'"));
        // The SECOND occurrence is flagged.
        assert_eq!(diags[0].span.start as usize, source.rfind('a').unwrap());
    }

    #[test]
    fn flags_duplicate_in_function_expression() {
        let diags = run("local f = function(x, x) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'x'"));
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
        assert!(diags[0].message.contains("'self'"));
    }

    #[test]
    fn flags_duplicate_in_global_function() {
        let diags = crate::test_support::run_rule(
            &DuplicateParameter,
            "global function f(a, a) end",
            LuaVersion::Lua55,
        );
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
