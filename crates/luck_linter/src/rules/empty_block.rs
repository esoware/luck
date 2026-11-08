use luck_ast::{Block, NodeType, NodeTypeSet, Statement};
use luck_token::Span;

use crate::diagnostic::{Category, LintDiagnostic, Severity};
use crate::rule::{LintContext, NodeRule, Rule};

#[derive(Debug, Clone, Copy)]
pub(crate) struct EmptyBlock;

impl Rule for EmptyBlock {
    fn name(&self) -> &'static str {
        "empty_block"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "block body is empty"
    }
}

impl NodeRule for EmptyBlock {
    fn node_types(&self) -> NodeTypeSet {
        NodeTypeSet::from_types(&[
            NodeType::IfStatement,
            NodeType::WhileLoop,
            NodeType::DoBlock,
            NodeType::NumericFor,
            NodeType::GenericFor,
            NodeType::RepeatLoop,
        ])
    }
    fn on_statement(
        &self,
        stmt: &Statement,
        _context: &LintContext<'_>,
        diags: &mut Vec<LintDiagnostic>,
    ) {
        let (block, message, span) = match stmt {
            Statement::IfStatement(if_stmt) => {
                diags.extend(check_body(&if_stmt.block, "empty `if` block", if_stmt.span));
                for clause in &if_stmt.elseif_clauses {
                    diags.extend(check_body(&clause.block, "empty `elseif` block", clause.span));
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    diags.extend(check_body(
                        &else_clause.block,
                        "empty `else` block",
                        else_clause.span,
                    ));
                }
                return;
            }
            Statement::WhileLoop(node) => (&node.block, "empty `while` loop body", node.span),
            Statement::DoBlock(node) => (&node.block, "empty `do` block", node.span),
            Statement::NumericFor(node) => {
                (&node.block, "empty numeric `for` loop body", node.span)
            }
            Statement::GenericFor(node) => {
                (&node.block, "empty generic `for` loop body", node.span)
            }
            Statement::RepeatLoop(node) => (&node.block, "empty `repeat` loop body", node.span),
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => return,
        };
        diags.extend(check_body(block, message, span));
    }
}

/// Flags `span` with `message` when `block` holds no statements.
fn check_body(block: &Block, message: &str, span: Span) -> Option<LintDiagnostic> {
    (block.stmts.is_empty() && block.last_stmt.is_none())
        .then(|| LintDiagnostic::new("empty_block", message, span))
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::run_rule;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        run_rule(&EmptyBlock, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_each_empty_if_branch() {
        let diags = run("local c = 1\nif c then elseif c then else end");
        let messages: Vec<&str> = diags.iter().map(|diag| diag.message.as_str()).collect();
        assert_eq!(
            messages,
            ["empty `if` block", "empty `elseif` block", "empty `else` block"],
            "{diags:?}"
        );
    }

    #[test]
    fn flags_empty_loop_and_do_bodies() {
        let diags = run(
            "local c = 1\nwhile c do end\ndo end\nfor i = 1, 2 do end\nfor k in pairs({}) do end\nrepeat until c",
        );
        let messages: Vec<&str> = diags.iter().map(|diag| diag.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "empty `while` loop body",
                "empty `do` block",
                "empty numeric `for` loop body",
                "empty generic `for` loop body",
                "empty `repeat` loop body",
            ],
            "{diags:?}"
        );
    }

    #[test]
    fn ignores_block_with_only_a_return() {
        let diags = run("do return end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_non_empty_blocks() {
        let diags = run("local c = 1\nif c then print(1) else print(2) end\nwhile c do break end");
        assert!(diags.is_empty(), "{diags:?}");
    }
}
