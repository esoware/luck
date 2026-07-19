use luck_ast::Statement;
use luck_ast::shared::Block;
use luck_ast::stmt::FuncName;
use luck_ast::visitor::Visitor;
use luck_token::{Span, TokenKind};

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, Rule};

pub struct DuplicateFunction;

impl Rule for DuplicateFunction {
    fn name(&self) -> &'static str {
        "duplicate_function"
    }

    fn category(&self) -> Category {
        Category::Suspicious
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Function with the same name is defined twice in the same block."
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let mut checker = DuplicateChecker {
            source: ctx.source,
            diagnostics: Vec::new(),
        };
        checker.visit_block(ctx.block);
        checker.diagnostics
    }
}

fn full_name(func_name: &FuncName) -> Option<String> {
    let mut parts = Vec::new();
    for token in &func_name.names {
        let TokenKind::Identifier(name) = &token.kind else {
            return None;
        };
        parts.push(name.as_str());
    }
    if parts.is_empty() {
        return None;
    }
    let mut joined = parts.join(".");
    if let Some((_, method_token)) = &func_name.method {
        let TokenKind::Identifier(method) = &method_token.kind else {
            return None;
        };
        joined.push(':');
        joined.push_str(method);
    }
    Some(joined)
}

fn line_of(source: &str, offset: Span) -> usize {
    let end = (offset.start as usize).min(source.len());
    source[..end].bytes().filter(|&byte| byte == b'\n').count() + 1
}

struct DuplicateChecker<'a> {
    source: &'a str,
    diagnostics: Vec<LintDiagnostic>,
}

impl DuplicateChecker<'_> {
    fn check_block(&mut self, block: &Block) {
        let mut defined: Vec<(String, Span)> = Vec::new();

        for stmt in &block.stmts {
            // Local functions are redefining_local's territory.
            let Statement::FunctionDecl(decl) = stmt else {
                continue;
            };
            let Some(name) = full_name(&decl.name) else {
                continue;
            };
            if let Some((_, first_span)) = defined.iter().find(|(n, _)| *n == name) {
                let line = line_of(self.source, *first_span);
                self.diagnostics.push(LintDiagnostic::new(
                    "duplicate_function",
                    format!("duplicate function definition '{name}'; also defined on line {line}"),
                    decl.name.span,
                ));
            } else {
                defined.push((name, decl.name.span));
            }
        }
    }
}

impl<'ast> Visitor<'ast> for DuplicateChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        self.walk_block(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&DuplicateFunction, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_duplicate_plain_function() {
        let diags = run("function f() end\nfunction f() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'f'"));
        assert!(diags[0].message.contains("line 1"));
    }

    #[test]
    fn flags_duplicate_dotted_function() {
        let diags = run("local t = {}\nfunction t.m() end\nfunction t.m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'t.m'"));
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_duplicate_method() {
        let diags = run("local t = {}\nfunction t:m() end\nfunction t:m() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("'t:m'"));
    }

    #[test]
    fn flags_duplicates_in_nested_block() {
        let diags = run("do\nfunction f() end\nfunction f() end\nend");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("line 2"));
    }

    #[test]
    fn flags_each_later_duplicate() {
        let diags = run("function f() end\nfunction f() end\nfunction f() end");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn ignores_dot_vs_colon() {
        let diags = run("local t = {}\nfunction t.m() end\nfunction t:m() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_different_names() {
        let diags = run("function f() end\nfunction g() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_local_functions() {
        let diags = run("local function f() end\nlocal function f() end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("do function f() end end\ndo function f() end end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
