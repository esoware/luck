use luck_ast::{
    Assignment, Block, CompoundAssignment, DoBlock, GenericFor, GotoStatement, IfStatement,
    LabelStatement, LastStatement, NumericFor, RepeatLoop, ReturnStatement, Statement, WhileLoop,
    stmt_starts_with_paren,
};

use super::CompactPrinter;
use crate::separator::PieceClass;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its emitters split by node family"
)]
impl CompactPrinter {
    pub(super) fn emit_block(&mut self, block: &Block) {
        for (i, stmt) in block.stmts.iter().enumerate() {
            // Without a `;`, a leading `(` would re-parse as a call on whatever
            // expression ends the previous statement.
            if i > 0 && stmt_starts_with_paren(stmt) && self.previous != PieceClass::Semicolon {
                self.emit_fixed(";");
            }
            self.emit_stmt(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.emit_last_stmt(last);
        }
    }

    fn emit_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(assignment) => self.emit_assignment(assignment),
            Statement::FunctionCall(call) => self.emit_function_call(call),
            Statement::DoBlock(block) => self.emit_do_block(block),
            Statement::WhileLoop(while_loop) => self.emit_while_loop(while_loop),
            Statement::RepeatLoop(repeat_loop) => self.emit_repeat_loop(repeat_loop),
            Statement::IfStatement(if_stmt) => self.emit_if_stmt(if_stmt),
            Statement::NumericFor(numeric_for) => self.emit_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.emit_generic_for(generic_for),
            Statement::FunctionDecl(declaration) => self.emit_function_declaration(declaration),
            Statement::LocalFunction(local_function) => self.emit_local_function(local_function),
            Statement::LocalAssignment(assignment) => self.emit_local_assignment(assignment),
            Statement::EmptyStatement(_) => self.emit_fixed(";"),
            // Lua 5.2+
            Statement::Goto(goto_stmt) => self.emit_goto(goto_stmt),
            // Lua 5.2+
            Statement::Label(label) => self.emit_label(label),
            // Lua 5.5
            Statement::GlobalDeclaration(declaration) => self.emit_global_declaration(declaration),
            // Lua 5.5
            Statement::GlobalFunction(function) => self.emit_global_function(function),
            // Lua 5.5
            Statement::GlobalStar(global_star) => self.emit_global_star(global_star),
            // Lua 5.2+
            Statement::Break(_) => self.emit_fixed("break"),
            // Luau
            Statement::CompoundAssignment(compound) => self.emit_compound_assignment(compound),
            // Luau
            Statement::TypeDeclaration(declaration) => self.emit_type_declaration(declaration),
            Statement::Error(_) => {}
        }
    }

    fn emit_last_stmt(&mut self, stmt: &LastStatement) {
        match stmt {
            LastStatement::Return(return_stmt) => self.emit_return(return_stmt),
            LastStatement::Break(_) => self.emit_fixed("break"),
            // Luau
            LastStatement::Continue(_) => self.emit_fixed("continue"),
            LastStatement::Error(_) => {}
        }
    }

    fn emit_assignment(&mut self, assignment: &Assignment) {
        self.emit_punctuated(&assignment.targets, ",", Self::emit_var);
        self.emit_fixed("=");
        self.emit_punctuated(&assignment.values, ",", Self::emit_expr);
    }

    fn emit_do_block(&mut self, block: &DoBlock) {
        self.emit_fixed("do");
        self.emit_block(&block.block);
        self.emit_fixed("end");
    }

    fn emit_while_loop(&mut self, while_loop: &WhileLoop) {
        self.emit_fixed("while");
        self.emit_expr(&while_loop.condition);
        self.emit_fixed("do");
        self.emit_block(&while_loop.block);
        self.emit_fixed("end");
    }

    fn emit_repeat_loop(&mut self, repeat_loop: &RepeatLoop) {
        self.emit_fixed("repeat");
        self.emit_block(&repeat_loop.block);
        self.emit_fixed("until");
        self.emit_expr(&repeat_loop.condition);
    }

    fn emit_if_stmt(&mut self, if_stmt: &IfStatement) {
        self.emit_fixed("if");
        self.emit_expr(&if_stmt.condition);
        self.emit_fixed("then");
        self.emit_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.emit_fixed("elseif");
            self.emit_expr(&clause.condition);
            self.emit_fixed("then");
            self.emit_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.emit_fixed("else");
            self.emit_block(&else_clause.block);
        }
        self.emit_fixed("end");
    }

    fn emit_numeric_for(&mut self, numeric_for: &NumericFor) {
        self.emit_fixed("for");
        self.emit_token(&numeric_for.name);
        if let Some(ty) = &numeric_for.type_annotation {
            self.emit_fixed(":");
            self.emit_type(ty);
        }
        self.emit_fixed("=");
        self.emit_expr(&numeric_for.start);
        self.emit_fixed(",");
        self.emit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.emit_fixed(",");
            self.emit_expr(step);
        }
        self.emit_fixed("do");
        self.emit_block(&numeric_for.block);
        self.emit_fixed("end");
    }

    fn emit_generic_for(&mut self, generic_for: &GenericFor) {
        self.emit_fixed("for");
        self.emit_punctuated(&generic_for.names, ",", Self::emit_param);
        self.emit_fixed("in");
        self.emit_punctuated(&generic_for.exprs, ",", Self::emit_expr);
        self.emit_fixed("do");
        self.emit_block(&generic_for.block);
        self.emit_fixed("end");
    }

    fn emit_goto(&mut self, goto_stmt: &GotoStatement) {
        self.emit_fixed("goto");
        self.emit_token(&goto_stmt.name);
    }

    fn emit_label(&mut self, label: &LabelStatement) {
        self.emit_fixed("::");
        self.emit_token(&label.name);
        self.emit_fixed("::");
    }

    fn emit_return(&mut self, return_stmt: &ReturnStatement) {
        self.emit_fixed("return");
        self.emit_punctuated(&return_stmt.exprs, ",", Self::emit_expr);
    }

    fn emit_compound_assignment(&mut self, compound: &CompoundAssignment) {
        self.emit_var(&compound.var);
        self.emit_fixed(compound.op.static_text());
        self.emit_expr(&compound.expr);
    }
}
