use luck_ast::shared::{Block, FunctionBody};
use luck_ast::stmt::LastStatement;
use luck_ast::visitor::Visitor;

use crate::diagnostic::{Category, Fix, LintDiagnostic, Severity, TextEdit};
use crate::rule::{LintContext, Rule};

pub struct RedundantReturn;

impl Rule for RedundantReturn {
    fn name(&self) -> &'static str {
        "redundant_return"
    }

    fn category(&self) -> Category {
        Category::Style
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Trailing bare return has no effect."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let mut checker = ReturnChecker {
            diagnostics: Vec::new(),
        };
        checker.check_tail(ctx.block);
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

struct ReturnChecker {
    diagnostics: Vec<LintDiagnostic>,
}

impl ReturnChecker {
    /// Flag a bare `return` closing `block` when `block` is a function
    /// body (or the chunk): falling off the end returns nothing anyway.
    /// Nested blocks are early returns - control flow, not redundancy.
    fn check_tail(&mut self, block: &Block) {
        let Some(last) = block.last_stmt.as_deref() else {
            return;
        };
        let LastStatement::Return(ret) = last else {
            return;
        };
        if !ret.exprs.is_empty() {
            return;
        }
        self.diagnostics.push(
            LintDiagnostic::new(
                "redundant_return",
                "bare return at the end of the function has no effect",
                ret.span,
            )
            .with_fix(Fix {
                description: "remove the redundant return".into(),
                edits: vec![TextEdit {
                    span: ret.span,
                    replacement: String::new(),
                }],
            }),
        );
    }
}

impl<'ast> Visitor<'ast> for ReturnChecker {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.check_tail(&body.block);
        self.walk_function_body(body);
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::RedundantReturn;
    use crate::diagnostic::LintDiagnostic;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&RedundantReturn, source, LuaVersion::Lua54)
    }

    fn apply_fix(source: &str, diag: &LintDiagnostic) -> String {
        let fix = diag.fix.as_ref().expect("fix");
        let edit = &fix.edits[0];
        let mut out = String::with_capacity(source.len());
        out.push_str(&source[..edit.span.start as usize]);
        out.push_str(&edit.replacement);
        out.push_str(&source[edit.span.end as usize..]);
        let parse = luck_parser::parse(&out, LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "reparse: {:?}", parse.errors);
        out
    }

    #[test]
    fn flags_trailing_bare_return_in_function() {
        let diags = run("local function f()\n    print(1)\n    return\nend\nf()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_trailing_bare_return_in_chunk() {
        let diags = run("print(1)\nreturn");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn flags_return_only_body() {
        let diags = run("local function f()\n    return\nend\nf()");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn fix_removes_return() {
        let source = "local function f()\n    print(1)\n    return\nend\nf()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply_fix(source, &diags[0]);
        assert!(!fixed.contains("return"), "{fixed}");
    }

    #[test]
    fn fix_handles_semicolon() {
        let source = "local function f()\n    print(1)\n    return;\nend\nf()";
        let diags = run(source);
        assert_eq!(diags.len(), 1, "{diags:?}");
        let fixed = apply_fix(source, &diags[0]);
        assert!(!fixed.contains("return"), "{fixed}");
    }

    #[test]
    fn ignores_return_with_values() {
        let diags = run("local function f()\n    return 1\nend\nf()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_early_return_in_nested_block() {
        let diags = run(
            "local function f(x)\n    if x then\n        return\n    end\n    print(x)\nend\nf(1)",
        );
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_nested_function_tail_return() {
        let diags = run(
            "local f = function()\n    local g = function()\n        return\n    end\n    return g\nend\nf()",
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
