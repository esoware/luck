use luck_ast::{Block, Statement};
use luck_token::{LuaVersion, Span, Token};

use crate::diagnostic::{Finding, line_column};
use crate::rule::{LintContext, WholeRule, impl_rule};

/// Flags a local declared again in the same block (Luacheck 411), which
/// hides the first one from all the code after it.
///
/// `shadowing` reports any redeclaration of a visible name, including ones
/// from enclosing blocks. Doing it in the same block is most often a
/// copy-paste bug, so it gets its own toggle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RedefiningLocal;

impl_rule!(
    RedefiningLocal,
    "redefining_local",
    Style,
    Warning,
    "local redeclared with the same name in the same block"
);

impl WholeRule for RedefiningLocal {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding> {
        let mut checker = RedefineChecker {
            source: context.source,
            version: context.semantic.version,
            declarations: Vec::new(),
            findings: Vec::new(),
        };
        for block in context.blocks {
            checker.check_block(block);
        }
        checker.findings
    }
}

struct RedefineChecker<'ast> {
    source: &'ast str,
    version: LuaVersion,
    declarations: Vec<(&'ast str, Span)>,
    findings: Vec<Finding>,
}

impl<'ast> RedefineChecker<'ast> {
    fn check_block(&mut self, block: &'ast Block) {
        self.declarations.clear();
        for stmt in &block.stmts {
            match stmt {
                Statement::LocalAssignment(local) => {
                    for attributed in local.names.iter() {
                        self.declare(&attributed.name);
                    }
                }
                Statement::LocalFunction(local_function) => self.declare(&local_function.name),
                Statement::Assignment(_)
                | Statement::FunctionCall(_)
                | Statement::DoBlock(_)
                | Statement::WhileLoop(_)
                | Statement::RepeatLoop(_)
                | Statement::IfStatement(_)
                | Statement::NumericFor(_)
                | Statement::GenericFor(_)
                | Statement::FunctionDecl(_)
                | Statement::EmptyStatement(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::Break(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
        }
    }

    fn declare(&mut self, name_token: &'ast Token) {
        let Some(name) = name_token.identifier() else {
            return;
        };
        // A leading `_` marks a name as deliberately unused.
        if name.starts_with('_') {
            return;
        }
        if let Some((_, first_span)) =
            self.declarations.iter().find(|(declared_name, _)| *declared_name == name)
        {
            let (line, column) = line_column(self.source, first_span.start, self.version);
            self.findings.push(
                Finding::new(
                    format!("local `{name}` is redeclared in the same block"),
                    name_token.span,
                )
                .with_help(format!("first declared at {line}:{column}")),
            );
        }
        self.declarations.push((name, name_token.span));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::LintDiagnostic;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&RedefiningLocal, source, LuaVersion::Lua54)
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
