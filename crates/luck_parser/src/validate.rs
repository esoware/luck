//! Post-parse validations that need scope context.
//!
//! Covers writes to read-only bindings, goto/label resolution, and Luau's
//! continue/until rule, all of which Lua rejects at compile time.

use luck_ast::{
    AttributedName, Block, FunctionBody, LastStatement, Punctuated, Statement, Type, Var, Visitor,
};
use luck_token::{LuaVersion, Span, Token};

use crate::ParseError;

/// Compile-time scope checks that Lua performs and [`parse`](crate::parse)
/// skips.
///
/// Covers writes to read-only bindings, goto/label resolution, and Luau's
/// continue/until rule. They need extra AST walks, so transform pipelines
/// skip them and `luck check` opts in. Run it only on a clean parse, since a
/// recovered AST produces misleading follow-on errors.
#[must_use]
pub fn validate(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    let mut errors = Vec::new();
    if version.has_attributes() || version.is_luau() {
        ConstWriteChecker { version, bindings: Vec::new(), errors: &mut errors }.visit_block(block);
    }
    if version.has_goto() {
        // Every function body's gotos, in walk order, then the main chunk's.
        GotoValidator { errors: &mut errors }.visit_block(block);
        resolve_gotos(block, &mut errors);
    }
    if version.has_continue() {
        ContinueUntilChecker { errors: &mut errors }.visit_block(block);
    }
    errors
}

/// Rejects writes to read-only bindings.
///
/// Those are `<const>`/`<close>` locals (5.4+), `<const>` globals (5.5), Luau
/// `const` bindings, and 5.5 for-loop control variables. Function boundaries
/// keep the scope stack, since Lua also rejects writes to a const upvalue.
struct ConstWriteChecker<'ast, 'errors> {
    version: LuaVersion,
    /// Visible names, innermost last, each with whether it's read-only.
    bindings: Vec<(&'ast str, bool)>,
    errors: &'errors mut Vec<ParseError>,
}

impl<'ast> ConstWriteChecker<'ast, '_> {
    fn declare(&mut self, token: &'ast Token, is_readonly: bool) {
        if let Some(name) = token.identifier() {
            self.bindings.push((name, is_readonly));
        }
    }

    fn declare_attributed_names(
        &mut self,
        names: &'ast Punctuated<AttributedName>,
        is_const_stmt: bool,
    ) {
        for attributed in names.iter() {
            let has_readonly_attribute = attributed.attribute.as_ref().is_some_and(|attribute| {
                matches!(attribute.name.identifier(), Some("const" | "close"))
            });
            self.declare(&attributed.name, is_const_stmt || has_readonly_attribute);
        }
    }

    fn is_readonly(&self, name: &str) -> Option<bool> {
        self.bindings
            .iter()
            .rev()
            .find_map(|(declared, is_readonly)| (*declared == name).then_some(*is_readonly))
    }

    fn check_write(&mut self, token: &Token) {
        if let Some(name) = token.identifier()
            && self.is_readonly(name) == Some(true)
        {
            self.errors.push(ParseError {
                span: token.span,
                message: format!("attempt to assign to const variable `{name}`"),
            });
        }
    }

    fn check_target(&mut self, target: &'ast Var) {
        match target {
            Var::Name(token) => self.check_write(token),
            // `t.x = 1` and `t[k] = 1` mutate the table, not the binding, and
            // const doesn't freeze contents.
            Var::FieldAccess(_) | Var::Index(_) => self.walk_var(target),
        }
    }
}

impl<'ast> Visitor<'ast> for ConstWriteChecker<'ast, '_> {
    fn visit_block(&mut self, block: &'ast Block) {
        let bindings_base = self.bindings.len();
        self.walk_block(block);
        self.bindings.truncate(bindings_base);
    }

    fn visit_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for value in assignment.values.iter() {
                    self.visit_expression(value);
                }
                for target in assignment.targets.iter() {
                    self.check_target(target);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound_assignment) => {
                self.visit_expression(&compound_assignment.expr);
                self.check_target(&compound_assignment.var);
            }
            Statement::NumericFor(numeric_for) => {
                self.visit_expression(&numeric_for.start);
                self.visit_expression(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expression(step);
                }
                let bindings_base = self.bindings.len();
                // Lua 5.5 makes for control variables read-only.
                self.declare(&numeric_for.name, self.version.has_const_for_variables());
                self.visit_block(&numeric_for.block);
                self.bindings.truncate(bindings_base);
            }
            Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.visit_expression(expr);
                }
                let bindings_base = self.bindings.len();
                let is_readonly = self.version.has_const_for_variables();
                for binding in generic_for.names.iter() {
                    self.declare(&binding.name, is_readonly);
                }
                self.visit_block(&generic_for.block);
                self.bindings.truncate(bindings_base);
            }
            Statement::FunctionDecl(function_decl) => {
                // `function a.b.c()` writes a field; `function a()` writes
                // the variable itself.
                if let [token] = function_decl.name.names.as_slice()
                    && function_decl.name.method.is_none()
                {
                    self.check_write(token);
                }
                self.visit_function_body(&function_decl.body);
            }
            Statement::LocalFunction(local_function) => {
                self.declare(&local_function.name, local_function.is_const);
                self.visit_function_body(&local_function.body);
            }
            Statement::LocalAssignment(local) => {
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
                self.declare_attributed_names(&local.names, local.is_const);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                if let Some(exprs) = &global.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
                self.declare_attributed_names(&global.names, false);
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::GlobalFunction(_)
            | Statement::TypeDeclaration(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_statement(stmt),
        }
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        let bindings_base = self.bindings.len();
        for param in body.params.iter() {
            self.declare(&param.name, false);
        }
        if let Some(vararg) = &body.vararg
            && let Some(name) = &vararg.name
        {
            self.declare(name, false);
        }
        self.walk_function_body(body);
        self.bindings.truncate(bindings_base);
    }

    // Types go unchecked, so a write inside a `typeof` is never reported.
    fn visit_type(&mut self, _type_value: &'ast Type) {}
}

// Goto/label resolution (5.2+), following the reference implementation.
// Each function body resolves its own labels. Duplicate visible labels and
// unresolved gotos are errors, and a forward goto may not jump into the scope
// of a local unless its label ends the block: only labels and `;` follow it,
// and the block isn't a repeat body, whose `until` still sees the locals.

struct PendingGoto<'ast> {
    name: &'ast str,
    span: Span,
    /// Locals declared in the current block before this goto. Rebased to the
    /// enclosing block's count when the goto moves outward.
    locals_below: usize,
}

/// One block's goto bookkeeping while [`validate_goto_block`] walks it.
struct GotoScope<'ast, 'state> {
    /// Labels of the enclosing blocks and of this block so far.
    visible_labels: &'state mut Vec<&'ast str>,
    errors: &'state mut Vec<ParseError>,
    pending_gotos: Vec<PendingGoto<'ast>>,
    locals_count: usize,
}

impl<'ast> GotoScope<'ast, '_> {
    /// Validate a nested block and fold its unresolved gotos into this
    /// block's pending list, rebasing their local count to this block's.
    fn absorb(&mut self, nested: &'ast Block, is_repeat_block: bool) {
        for mut goto in
            validate_goto_block(nested, self.visible_labels, is_repeat_block, self.errors)
        {
            goto.locals_below = self.locals_count;
            self.pending_gotos.push(goto);
        }
    }
}

/// Finds every function body so each gets its own label scope. The caller
/// resolves the main chunk's gotos itself.
struct GotoValidator<'errors> {
    errors: &'errors mut Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for GotoValidator<'_> {
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        resolve_gotos(&body.block, self.errors);
        self.walk_function_body(body);
    }
}

/// Validate the gotos of a function body or the main chunk, reporting each
/// one that no label in it takes.
fn resolve_gotos(block: &Block, errors: &mut Vec<ParseError>) {
    for goto in validate_goto_block(block, &mut Vec::new(), false, errors) {
        errors.push(ParseError {
            span: goto.span,
            message: format!("no visible label `{}` for `goto`", goto.name),
        });
    }
}

/// Validate one block, returning gotos that must resolve in an enclosing
/// block.
///
/// `visible_labels` holds the enclosing blocks' labels, for backward jumps and
/// duplicate detection, and is restored to its original length on return.
fn validate_goto_block<'ast>(
    block: &'ast Block,
    visible_labels: &mut Vec<&'ast str>,
    is_repeat_block: bool,
    errors: &mut Vec<ParseError>,
) -> Vec<PendingGoto<'ast>> {
    let visible_labels_base = visible_labels.len();
    let mut scope =
        GotoScope { visible_labels, errors, pending_gotos: Vec::new(), locals_count: 0 };

    // A repeat body's `until` still sees the body's locals, so its trailing
    // labels don't count as the end of the block.
    let trailing_void_start = if is_repeat_block {
        block.stmts.len()
    } else {
        block
            .stmts
            .iter()
            .rposition(|stmt| !matches!(stmt, Statement::Label(_) | Statement::EmptyStatement(_)))
            .map_or(0, |index| index.saturating_add(1))
    };

    for (stmt_index, stmt) in block.stmts.iter().enumerate() {
        match stmt {
            // Lua 5.2+
            Statement::Label(label) => {
                if let Some(name) = label.name.identifier() {
                    if scope.visible_labels.contains(&name) {
                        scope.errors.push(ParseError {
                            span: label.span,
                            message: format!("label `{name}` already defined"),
                        });
                    }
                    // Pending gotos with this name are forward jumps to this
                    // label. Each one fails if a local was declared in
                    // between, unless the label ends the block.
                    let locals_count = scope.locals_count;
                    scope.pending_gotos.retain(|goto| {
                        if goto.name != name {
                            return true;
                        }
                        if goto.locals_below < locals_count && stmt_index < trailing_void_start {
                            scope.errors.push(ParseError {
                                span: goto.span,
                                message: format!("`goto {name}` jumps into the scope of a local"),
                            });
                        }
                        false
                    });
                    scope.visible_labels.push(name);
                }
            }
            // Lua 5.2+
            Statement::Goto(goto) => {
                // A visible label makes this a backward jump, resolved now.
                if let Some(name) = goto.name.identifier()
                    && !scope.visible_labels.contains(&name)
                {
                    scope.pending_gotos.push(PendingGoto {
                        name,
                        span: goto.span,
                        locals_below: scope.locals_count,
                    });
                }
            }
            Statement::LocalAssignment(local) => {
                scope.locals_count = scope.locals_count.saturating_add(local.names.len());
            }
            Statement::LocalFunction(_) => {
                scope.locals_count = scope.locals_count.saturating_add(1);
            }
            Statement::DoBlock(do_block) => scope.absorb(&do_block.block, false),
            Statement::WhileLoop(while_loop) => scope.absorb(&while_loop.block, false),
            Statement::RepeatLoop(repeat_loop) => scope.absorb(&repeat_loop.block, true),
            Statement::IfStatement(if_stmt) => {
                scope.absorb(&if_stmt.block, false);
                for clause in &if_stmt.elseif_clauses {
                    scope.absorb(&clause.block, false);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    scope.absorb(&else_clause.block, false);
                }
            }
            Statement::NumericFor(numeric_for) => scope.absorb(&numeric_for.block, false),
            Statement::GenericFor(generic_for) => scope.absorb(&generic_for.block, false),
            // Function bodies get their own label scope via `GotoValidator`.
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

    scope.visible_labels.truncate(visible_labels_base);
    scope.pending_gotos
}

// Luau: a `continue` in a repeat body may not skip the declaration of a local
// that the `until` condition reads.

struct ContinueUntilChecker<'errors> {
    errors: &'errors mut Vec<ParseError>,
}

impl<'ast> Visitor<'ast> for ContinueUntilChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast Statement) {
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
                reads.visit_expression(&repeat_loop.condition);
                for name in reads.names {
                    if skipped_names.contains(&name) {
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
        self.walk_statement(stmt);
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
