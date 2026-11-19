//! Luau: a `continue` in a repeat body may not skip the declaration of a local
//! that the `until` condition reads.

use luck_ast::{Block, Statement};

/// How many locals `repeat_body` declares after its first `continue`, which
/// are the top-level locals the `until` condition may not read.
pub(super) fn skipped_local_count(repeat_body: &Block<'_>) -> usize {
    let Some(continue_index) = earliest_continue_stmt(repeat_body) else {
        return 0;
    };
    let after_continue = continue_index.saturating_add(1);
    repeat_body
        .stmts
        .iter()
        .skip(after_continue)
        .map(|later| match later {
            Statement::LocalAssignment(local) => local.names.len(),
            Statement::LocalFunction(_) => 1,
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
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Error(_) => 0,
        })
        .fold(0, usize::saturating_add)
}

/// Index of the first top-level statement of `block` that contains a
/// `continue` belonging to the enclosing repeat.
///
/// Continues inside nested loops and function bodies don't count.
fn earliest_continue_stmt(block: &Block<'_>) -> Option<usize> {
    block.stmts.iter().position(stmt_has_binding_continue)
}

fn block_has_binding_continue(block: &Block<'_>) -> bool {
    block.stmts.iter().any(stmt_has_binding_continue)
}

fn stmt_has_binding_continue(stmt: &Statement<'_>) -> bool {
    match stmt {
        // Luau
        Statement::Continue(_) => true,
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
        | Statement::Return(_)
        | Statement::Break(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalDeclaration(_)
        | Statement::GlobalStar(_)
        | Statement::CompoundAssignment(_)
        | Statement::TypeDeclaration(_)
        | Statement::Error(_) => false,
    }
}
