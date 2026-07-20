//! Post-parse validations that need scope context: writes to read-only
//! bindings, goto/label resolution, and Luau's continue/until rule.
//! Real Lua rejects all of these at compile time, so accepting them
//! would let every downstream consumer transform invalid programs.

use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::stmt::*;
use luck_ast::visitor::Visitor;
use luck_ast::{Expression, LastStatement, Statement};
use luck_token::{LuaVersion, Span, TokenKind};

use crate::ParseError;

pub(crate) fn validate(block: &Block, version: LuaVersion, errors: &mut Vec<ParseError>) {
    if version.has_attributes() || version.is_luau() {
        let mut checker = ConstWriteChecker {
            version,
            scopes: vec![Vec::new()],
            errors,
        };
        checker.check_block(block);
    }
    if version.has_goto() {
        let mut collector = GotoValidator { errors };
        collector.visit_block(block);
        let unresolved = validate_goto_block(block, &mut Vec::new(), false, collector.errors);
        for goto in unresolved {
            collector.errors.push(ParseError {
                span: goto.span,
                message: format!("no visible label '{}' for goto", goto.name),
            });
        }
    }
    if version.is_luau() {
        let mut checker = ContinueUntilChecker { errors };
        checker.visit_block(block);
    }
}

fn ident_text(token: &luck_token::Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Identifier(name) => Some(name.as_str()),
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Writes to read-only bindings: 5.4/5.5 `<const>`/`<close>` locals,
// Luau `const` bindings, and 5.5 for-loop control variables. Function
// boundaries do NOT reset the scope stack - real Lua also rejects
// upvalue writes to const bindings.
// ---------------------------------------------------------------------

struct ConstWriteChecker<'a> {
    version: LuaVersion,
    /// One frame per block/function scope: (name, is_readonly).
    scopes: Vec<Vec<(String, bool)>>,
    errors: &'a mut Vec<ParseError>,
}

impl ConstWriteChecker<'_> {
    fn declare(&mut self, token: &luck_token::Token, readonly: bool) {
        if let Some(name) = ident_text(token) {
            self.scopes
                .last_mut()
                .expect("scope stack is never empty")
                .push((name.to_string(), readonly));
        }
    }

    fn is_readonly(&self, name: &str) -> Option<bool> {
        for frame in self.scopes.iter().rev() {
            for (declared, readonly) in frame.iter().rev() {
                if declared == name {
                    return Some(*readonly);
                }
            }
        }
        None
    }

    fn check_write(&mut self, token: &luck_token::Token, span: Span) {
        if let Some(name) = ident_text(token)
            && self.is_readonly(name) == Some(true)
        {
            self.errors.push(ParseError {
                span,
                message: format!("attempt to assign to const variable '{name}'"),
            });
        }
    }

    fn attributed_name_is_readonly(
        &self,
        attributed: &AttributedName,
        is_const_stmt: bool,
    ) -> bool {
        if is_const_stmt {
            return true;
        }
        attributed
            .attrib
            .as_ref()
            .is_some_and(|attrib| matches!(ident_text(&attrib.name), Some("const") | Some("close")))
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(Vec::new());
        self.check_block_in_current_scope(block);
        self.scopes.pop();
    }

    fn check_block_in_current_scope(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.check_statement(stmt);
        }
        if let Some(last) = &block.last_stmt {
            if let LastStatement::Return(ret) = last.as_ref() {
                for expr in ret.exprs.iter() {
                    self.check_expression(expr);
                }
            }
        }
    }

    fn check_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    self.check_expression(expr);
                }
                for target in assign.targets.iter() {
                    match target {
                        Var::Name(token) => self.check_write(token, token.span),
                        // `t.x = 1` / `t[k] = 1` mutate the table, not the
                        // binding - const does not freeze contents.
                        Var::FieldAccess(_) | Var::Index(_) => self.check_var_reads(target),
                    }
                }
            }
            Statement::CompoundAssignment(compound) => {
                self.check_expression(&compound.expr);
                match &compound.var {
                    Var::Name(token) => self.check_write(token, token.span),
                    other => self.check_var_reads(other),
                }
            }
            Statement::FunctionCall(call) => self.check_call(&call.call),
            Statement::DoBlock(do_block) => self.check_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.check_expression(&while_loop.condition);
                self.check_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.check_block(&repeat_loop.block);
                self.check_expression(&repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                self.check_expression(&if_stmt.condition);
                self.check_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.check_expression(&clause.condition);
                    self.check_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.check_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                self.check_expression(&numeric_for.start);
                self.check_expression(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.check_expression(step);
                }
                self.scopes.push(Vec::new());
                // Lua 5.5 makes for control variables read-only.
                self.declare(&numeric_for.name, self.version.has_const_for_variables());
                self.check_block_in_current_scope(&numeric_for.block);
                self.scopes.pop();
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.check_expression(expr);
                }
                self.scopes.push(Vec::new());
                let readonly = self.version.has_const_for_variables();
                for binding in generic_for.names.iter() {
                    self.declare(&binding.name, readonly);
                }
                self.check_block_in_current_scope(&generic_for.block);
                self.scopes.pop();
            }
            Statement::FunctionDecl(decl) => {
                // `function a.b.c()` writes a field; `function a()` writes
                // the variable itself.
                if decl.name.names.len() == 1 && decl.name.method.is_none() {
                    let token = &decl.name.names[0];
                    self.check_write(token, token.span);
                }
                self.check_function_body(&decl.body);
            }
            Statement::LocalFunction(func) => {
                self.declare(&func.name, func.is_const);
                self.check_function_body(&func.body);
            }
            Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.check_expression(expr);
                    }
                }
                for attributed in local.names.iter() {
                    let readonly = self.attributed_name_is_readonly(attributed, local.is_const);
                    self.declare(&attributed.name, readonly);
                }
            }
            Statement::GlobalDeclaration(global) => {
                if let Some(exprs) = &global.exprs {
                    for expr in exprs.iter() {
                        self.check_expression(expr);
                    }
                }
                for attributed in global.names.iter() {
                    let readonly = self.attributed_name_is_readonly(attributed, false);
                    self.declare(&attributed.name, readonly);
                }
            }
            Statement::GlobalFunction(func) => self.check_function_body(&func.body),
            Statement::TypeDeclaration(type_decl) => {
                if let TypeDeclarationValue::TypeFunction(body) = &type_decl.type_value {
                    self.check_function_body(body);
                }
            }
            Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    fn check_function_body(&mut self, body: &FunctionBody) {
        self.scopes.push(Vec::new());
        for param in body.params.iter() {
            self.declare(&param.name, false);
        }
        if let Some(vararg) = &body.vararg
            && let Some(name) = &vararg.name
        {
            self.declare(name, false);
        }
        self.check_block_in_current_scope(&body.block);
        self.scopes.pop();
    }

    fn check_var_reads(&mut self, var: &Var) {
        match var {
            Var::Name(_) => {}
            Var::FieldAccess(access) => self.check_expression(&access.prefix),
            Var::Index(index) => {
                self.check_expression(&index.prefix);
                self.check_expression(&index.index);
            }
        }
    }

    fn check_call(&mut self, call: &FunctionCall) {
        self.check_expression(&call.callee);
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for expr in args.iter() {
                    self.check_expression(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => self.check_table(table),
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn check_table(&mut self, table: &TableConstructor) {
        for field in table.fields.iter() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    self.check_expression(key);
                    self.check_expression(value);
                }
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    self.check_expression(value);
                }
            }
        }
    }

    fn check_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
            Expression::FunctionDef(def) => self.check_function_body(&def.body),
            Expression::Var(var) => self.check_var_reads(var),
            Expression::FunctionCall(call) => self.check_call(call),
            Expression::Parenthesized(paren) => self.check_expression(&paren.expr),
            Expression::TableConstructor(table) => self.check_table(table),
            Expression::BinaryOp(binop) => {
                self.check_expression(&binop.left);
                self.check_expression(&binop.right);
            }
            Expression::UnaryOp(unop) => self.check_expression(&unop.operand),
            Expression::IfExpression(if_expr) => {
                self.check_expression(&if_expr.condition);
                self.check_expression(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.check_expression(&clause.condition);
                    self.check_expression(&clause.expr);
                }
                self.check_expression(&if_expr.else_expr);
            }
            Expression::InterpolatedString(interp) => {
                for segment in &interp.segments {
                    if let Some(expr) = &segment.expr {
                        self.check_expression(expr);
                    }
                }
            }
            Expression::TypeCast(cast) => self.check_expression(&cast.expr),
        }
    }
}

// ---------------------------------------------------------------------
// Goto/label resolution (5.2+). Each function body resolves its own
// labels; gotos never cross function boundaries. Mirrors PUC rules:
// duplicate visible labels error, an unresolved goto errors, and a
// forward goto may not enter the scope of a local unless the target
// label sits at the end of its block (followed only by labels/`;`,
// and not directly before `until`).
// ---------------------------------------------------------------------

struct PendingGoto {
    name: String,
    span: Span,
    /// Number of locals declared in the CURRENT block before this goto
    /// appeared (reset as it propagates outward).
    locals_below: usize,
}

/// Finds every function body so each gets its own label scope. The main
/// chunk's block is validated by the caller directly.
struct GotoValidator<'a> {
    errors: &'a mut Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for GotoValidator<'_> {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        let unresolved = validate_goto_block(&body.block, &mut Vec::new(), false, self.errors);
        for goto in unresolved {
            self.errors.push(ParseError {
                span: goto.span,
                message: format!("no visible label '{}' for goto", goto.name),
            });
        }
        self.walk_function_body(body);
    }
}

/// Validate one block, returning gotos that must resolve in an
/// enclosing block. `visible` carries labels of enclosing blocks (for
/// backward jumps and duplicate detection) and is truncated on exit.
fn validate_goto_block(
    block: &Block,
    visible: &mut Vec<String>,
    is_repeat_block: bool,
    errors: &mut Vec<ParseError>,
) -> Vec<PendingGoto> {
    let visible_base = visible.len();
    let mut locals_count = 0usize;
    // (name, locals before it, statement index)
    let mut labels_here: Vec<(String, usize, usize)> = Vec::new();
    let mut pending: Vec<PendingGoto> = Vec::new();

    // suffix_void[i]: statements i.. are all labels/`;` (so a label at i
    // is "at the end of the block" per PUC's skipnoopstat rule). A repeat
    // block's trailing label still precedes `until`, which CAN see the
    // block's locals, so it never counts as at-the-end there.
    let mut suffix_void = vec![false; block.stmts.len() + 1];
    suffix_void[block.stmts.len()] = !is_repeat_block;
    for idx in (0..block.stmts.len()).rev() {
        suffix_void[idx] = suffix_void[idx + 1]
            && matches!(
                block.stmts[idx],
                Statement::Label(_) | Statement::EmptyStatement(_)
            );
    }

    for (idx, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            Statement::Label(label) => {
                if let Some(name) = ident_text(&label.name) {
                    if labels_here.iter().any(|(existing, _, _)| existing == name)
                        || visible.iter().any(|existing| existing == name)
                    {
                        errors.push(ParseError {
                            span: label.span,
                            message: format!("label '{name}' already defined"),
                        });
                    }
                    // A label resolves every pending goto with its name:
                    // those jumps are backward from here on, or forward
                    // jumps whose scope entry must be checked.
                    pending.retain(|goto| {
                        if goto.name != name {
                            return true;
                        }
                        if goto.locals_below < locals_count && !suffix_void[idx] {
                            errors.push(ParseError {
                                span: goto.span,
                                message: format!("goto '{name}' jumps into the scope of a local"),
                            });
                        }
                        false
                    });
                    labels_here.push((name.to_string(), locals_count, idx));
                    visible.push(name.to_string());
                }
            }
            Statement::Goto(goto) => {
                if let Some(name) = ident_text(&goto.name) {
                    let backward = labels_here.iter().any(|(existing, _, _)| existing == name)
                        || visible[..visible_base]
                            .iter()
                            .any(|existing| existing == name);
                    if !backward {
                        pending.push(PendingGoto {
                            name: name.to_string(),
                            span: goto.span,
                            locals_below: locals_count,
                        });
                    }
                }
            }
            Statement::LocalAssignment(local) => {
                locals_count += local.names.len();
            }
            Statement::LocalFunction(_) => {
                locals_count += 1;
            }
            Statement::DoBlock(do_block) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&do_block.block, visible, false, errors),
                    locals_count,
                );
            }
            Statement::WhileLoop(while_loop) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&while_loop.block, visible, false, errors),
                    locals_count,
                );
            }
            Statement::RepeatLoop(repeat_loop) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&repeat_loop.block, visible, true, errors),
                    locals_count,
                );
            }
            Statement::IfStatement(if_stmt) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&if_stmt.block, visible, false, errors),
                    locals_count,
                );
                for clause in &if_stmt.elseif_clauses {
                    absorb(
                        &mut pending,
                        validate_goto_block(&clause.block, visible, false, errors),
                        locals_count,
                    );
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    absorb(
                        &mut pending,
                        validate_goto_block(&else_clause.block, visible, false, errors),
                        locals_count,
                    );
                }
            }
            Statement::NumericFor(numeric_for) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&numeric_for.block, visible, false, errors),
                    locals_count,
                );
            }
            Statement::GenericFor(generic_for) => {
                absorb(
                    &mut pending,
                    validate_goto_block(&generic_for.block, visible, false, errors),
                    locals_count,
                );
            }
            // Function bodies get their own label scope via GotoValidator.
            Statement::Assignment(_)
            | Statement::CompoundAssignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    visible.truncate(visible_base);
    pending
}

/// Fold a nested block's unresolved gotos into the parent's pending
/// list, rebasing their local count to the parent block's.
fn absorb(pending: &mut Vec<PendingGoto>, nested: Vec<PendingGoto>, locals_count: usize) {
    for mut goto in nested {
        goto.locals_below = locals_count;
        pending.push(goto);
    }
}

// ---------------------------------------------------------------------
// Luau: `continue` in repeat..until may not jump over the declaration
// of a local that the until condition uses.
// ---------------------------------------------------------------------

struct ContinueUntilChecker<'a> {
    errors: &'a mut Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for ContinueUntilChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        if let Statement::RepeatLoop(repeat_loop) = stmt {
            if let Some(continue_idx) = earliest_continue_stmt(&repeat_loop.block) {
                let mut skipped: Vec<&str> = Vec::new();
                for later in repeat_loop.block.stmts.iter().skip(continue_idx + 1) {
                    match later {
                        Statement::LocalAssignment(local) => {
                            skipped.extend(local.names.iter().filter_map(|n| ident_text(&n.name)));
                        }
                        Statement::LocalFunction(func) => {
                            skipped.extend(ident_text(&func.name));
                        }
                        _ => {}
                    }
                }
                if !skipped.is_empty() {
                    let mut reads = ReadCollector { names: Vec::new() };
                    reads.visit_expression(&repeat_loop.condition);
                    for name in reads.names {
                        if skipped.contains(&name.as_str()) {
                            self.errors.push(ParseError {
                                span: repeat_loop.condition.span(),
                                message: format!(
                                    "local '{name}' used in the until condition is declared after a continue statement"
                                ),
                            });
                        }
                    }
                }
            }
        }
        self.walk_statement(stmt);
    }
}

/// Index of the first top-level statement of `block` whose subtree holds
/// a `continue` binding to the enclosing repeat (nested loops capture
/// their own continues; function bodies are separate). A continue as the
/// block's own last statement returns the index past the last stmt.
fn earliest_continue_stmt(block: &Block) -> Option<usize> {
    for (idx, stmt) in block.stmts.iter().enumerate() {
        if stmt_has_binding_continue(stmt) {
            return Some(idx);
        }
    }
    if matches!(block.last_stmt.as_deref(), Some(LastStatement::Continue(_))) {
        return Some(block.stmts.len());
    }
    None
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

struct ReadCollector {
    names: Vec<String>,
}

impl<'ast> Visitor<'ast> for ReadCollector {
    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(token) = var
            && let Some(name) = ident_text(token)
        {
            self.names.push(name.to_string());
        }
        self.walk_var(var);
    }
}
