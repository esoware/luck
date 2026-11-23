use luck_ast::{Block, Name, Statement};
use luck_token::{MAX_ACTIVE_LOCALS, Span};

use crate::diagnostic::Finding;
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
        let mut checker = RedefineChecker { declarations: Vec::new(), findings: Vec::new() };
        for block in context.nodes.blocks() {
            checker.check_block(block);
        }
        checker.findings
    }
}

struct RedefineChecker<'ast> {
    /// Each name the block declares, in order. Every one stays active until
    /// the block ends, so no Lua compiles a block with more than
    /// [`MAX_ACTIVE_LOCALS`], and the rest of one isn't checked.
    declarations: Vec<(&'ast str, Span)>,
    findings: Vec<Finding>,
}

impl<'ast> RedefineChecker<'ast> {
    fn check_block(&mut self, block: &'ast Block<'ast>) {
        self.declarations.clear();
        for stmt in &block.stmts {
            match stmt {
                Statement::LocalAssignment(local) => {
                    for attributed in &local.names {
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
                | Statement::Return(_)
                | Statement::Goto(_)
                | Statement::Label(_)
                | Statement::GlobalDeclaration(_)
                | Statement::GlobalFunction(_)
                | Statement::GlobalStar(_)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::CompoundAssignment(_)
                | Statement::TypeDeclaration(_)
                | Statement::Error(_) => {}
            }
        }
    }

    fn declare(&mut self, declared: &Name<'ast>) {
        let name = declared.text.as_str();
        if self.declarations.len() > MAX_ACTIVE_LOCALS {
            return;
        }
        // A leading `_` marks a name as deliberately unused.
        if name.starts_with('_') {
            return;
        }
        if let Some(&(_, first_span)) =
            self.declarations.iter().find(|(declared_name, _)| *declared_name == name)
        {
            self.findings.push(
                Finding::new(
                    format!("local `{name}` is redeclared in the same block"),
                    declared.span,
                )
                .with_label(first_span, "first declared here"),
            );
        }
        self.declarations.push((name, declared.span));
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_token::LuaVersion;

    use super::*;
    use crate::diagnostic::Message;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<Message> {
        run_rule(&RedefiningLocal, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_same_block_redeclaration() {
        let diags = run("\
do local x = 1
local x = 2
print(x) end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
        assert!(diags[0].message.contains("`x`"));
        assert_eq!(diags[0].labels.first().map(|label| label.span.start), Some(9));
    }

    #[test]
    fn ignores_sibling_blocks() {
        let diags = run("\
do local x = 1 end
do local x = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn ignores_nested_block_shadowing() {
        // Inner block shadowing is `shadowing`'s job.
        let diags = run("\
local x = 1
do local x = 2 end");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    #[test]
    fn flags_multiple_redeclarations() {
        let diags = run("\
local x = 1
local x = 2
local x = 3");
        assert_eq!(diags.len(), 2, "got: {diags:?}");
    }

    #[test]
    fn flags_local_function_redeclaration() {
        let diags = run("\
local function f() end
local function f() end");
        assert_eq!(diags.len(), 1, "got: {diags:?}");
    }

    #[test]
    fn ignores_underscore_names() {
        let diags = run("\
local _ = 1
local _ = 2");
        assert!(diags.is_empty(), "got: {diags:?}");
    }

    fn locals_declared_twice(count: u32) -> String {
        let mut locals = String::new();
        for i in 0..count {
            let _unused = writeln!(locals, "local x{i} = 1");
        }
        format!("{locals}{locals}")
    }

    #[test]
    fn flags_each_redeclaration_in_a_block_at_the_local_limit() {
        let diags = run(&locals_declared_twice(100));
        assert_eq!(diags.len(), 100, "{diags:?}");
        assert_eq!(
            diags[0].labels.first().map(|label| label.span.start),
            Some(6),
            "{:?}",
            diags[0]
        );
    }

    #[test]
    fn ignores_the_rest_of_a_block_past_the_local_limit() {
        let diags = run(&locals_declared_twice(201));
        assert!(diags.is_empty(), "{diags:?}");
    }
}
