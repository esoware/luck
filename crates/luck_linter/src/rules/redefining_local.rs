use luck_ast::Statement;
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::{Span, TokenKind};

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// Luacheck 411-413: a local is declared with the same name as another
/// local *in the same block*. The second declaration makes the first
/// effectively unreachable.
///
/// Why this is distinct from `shadowing`: `shadowing` covers outer-scope
/// shadowing (`local x = 1; do local x = 2 end`). Redefining within the
/// same block is a different pattern - most often a copy-paste bug -
/// and benefits from its own toggle.
pub struct RedefiningLocal;

impl Rule for RedefiningLocal {
    fn name(&self) -> &'static str {
        "redefining_local"
    }
    fn category(&self) -> Category {
        Category::Style
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "local redeclared with the same name in the same block"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let mut checker = RedefineChecker {
            source: ctx.source,
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        checker.diagnostics
    }
}

struct RedefineChecker<'a> {
    source: &'a str,
    diagnostics: Vec<LintDiagnostic>,
}

impl RedefineChecker<'_> {
    fn check_block(&mut self, block: &Block) {
        // (name, definition_span) for every local already declared in
        // this block. Order matters - we report later declarations.
        let mut declared: Vec<(String, Span)> = Vec::new();

        for stmt in &block.stmts {
            match stmt {
                Statement::LocalAssignment(local) => {
                    for attributed in local.names.iter() {
                        self.note_local(&mut declared, &attributed.name);
                    }
                }
                Statement::LocalFunction(func) => {
                    self.note_local(&mut declared, &func.name);
                }
                _ => {}
            }
        }
    }

    fn note_local(&mut self, declared: &mut Vec<(String, Span)>, name_token: &luck_token::Token) {
        let TokenKind::Identifier(name) = &name_token.kind else {
            return;
        };
        // Underscore convention: don't flag intentional throwaway names.
        if name == "_" || name.starts_with('_') {
            declared.push((name.to_string(), name_token.span));
            return;
        }
        if let Some((_, first_span)) = declared.iter().find(|(n, _)| n == name.as_str()) {
            let (line, column) = luck_token::line_col(self.source, first_span.start);
            self.diagnostics.push(
                LintDiagnostic::new(
                    "redefining_local",
                    format!("local `{name}` is redeclared in the same block"),
                    name_token.span,
                )
                .with_help(format!("previous declaration at {line}:{column}")),
            );
        }
        declared.push((name.to_string(), name_token.span));
    }
}

impl<'ast> Visitor<'ast> for RedefineChecker<'_> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.check_block(block);
        // Recurse into every nested block via the default walk so that
        // inner blocks (if/while/for/do/function bodies) get their own
        // independent same-block check.
        self.walk_block(block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&RedefiningLocal, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_same_block_redeclaration() {
        let diags = run("do local x = 1\nlocal x = 2\nprint(x) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("do local x = 1 end\ndo local x = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_nested_block_shadowing() {
        // Inner block shadowing is `shadowing`'s job.
        let diags = run("local x = 1\ndo local x = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_multiple_redeclarations() {
        let diags = run("local x = 1\nlocal x = 2\nlocal x = 3");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_local_function_redeclaration() {
        let diags = run("local function f() end\nlocal function f() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_names() {
        let diags = run("local _ = 1\nlocal _ = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }
}
