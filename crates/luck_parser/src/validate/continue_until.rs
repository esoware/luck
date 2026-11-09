//! Luau: a `continue` in a repeat body may not skip the declaration of a local
//! that the `until` condition reads.

use luck_ast::{Block, LastStatement, Statement, Var, Visitor};

use crate::ParseError;

pub(super) fn check(block: &Block) -> Vec<ParseError> {
    let mut checker = ContinueUntilChecker { errors: Vec::new() };
    checker.visit_block(block);
    checker.errors
}

struct ContinueUntilChecker {
    errors: Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for ContinueUntilChecker {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        if let Statement::RepeatLoop(repeat_loop) = stmt
            && let Some(continue_index) = earliest_continue_stmt(&repeat_loop.block)
        {
            let mut skipped_names: Vec<&str> = Vec::new();
            let after_continue = continue_index.saturating_add(1);
            for later in repeat_loop.block.stmts.iter().skip(after_continue) {
                match later {
                    Statement::LocalAssignment(local) => {
                        for attributed in local.names.iter() {
                            skipped_names.extend(attributed.name.identifier());
                        }
                    }
                    Statement::LocalFunction(local_function) => {
                        skipped_names.extend(local_function.name.identifier());
                    }
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
            if !skipped_names.is_empty() {
                let mut reads = ReadCollector { names: Vec::new() };
                reads.visit_expr(&repeat_loop.condition);
                for name in reads.names {
                    if skipped_names.contains(&name) {
                        skipped_names.retain(|skipped| *skipped != name);
                        self.errors.push(ParseError {
                            span: repeat_loop.condition.span(),
                            message: format!(
                                "local `{name}` used in the `until` condition is declared after a `continue` statement"
                            ),
                        });
                    }
                }
            }
        }
        self.walk_stmt(stmt);
    }
}

/// Index of the first top-level statement of `block` that contains a
/// `continue` belonging to the enclosing repeat.
///
/// Continues inside nested loops and function bodies don't count. A trailing
/// `continue` returns `block.stmts.len()`.
fn earliest_continue_stmt(block: &Block) -> Option<usize> {
    block.stmts.iter().position(stmt_has_binding_continue).or_else(|| {
        matches!(block.last_stmt.as_deref(), Some(LastStatement::Continue(_)))
            .then_some(block.stmts.len())
    })
}

fn block_has_binding_continue(block: &Block) -> bool {
    matches!(block.last_stmt.as_deref(), Some(LastStatement::Continue(_)))
        || block.stmts.iter().any(stmt_has_binding_continue)
}

fn stmt_has_binding_continue(stmt: &Statement) -> bool {
    match stmt {
        Statement::DoBlock(do_block) => block_has_binding_continue(&do_block.block),
        Statement::IfStatement(if_stmt) => {
            block_has_binding_continue(&if_stmt.block)
                || if_stmt
                    .elseif_clauses
                    .iter()
                    .any(|clause| block_has_binding_continue(&clause.block))
                || if_stmt
                    .else_clause
                    .as_ref()
                    .is_some_and(|clause| block_has_binding_continue(&clause.block))
        }
        Statement::WhileLoop(_)
        | Statement::RepeatLoop(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::GlobalFunction(_)
        | Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::LocalAssignment(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalStar(_)
        | Statement::CompoundAssignment(_)
        | Statement::Break(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}

/// Every name the visited code reads, in walk order, repeats included.
struct ReadCollector<'ast> {
    names: Vec<&'ast str>,
}

impl<'ast> Visitor<'ast> for ReadCollector<'ast> {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(token) = var
            && let Some(name) = token.identifier()
        {
            self.names.push(name);
        }
        self.walk_var(var);
    }
}
