//! One walk over a chunk's active variables, the stack `lparser.c` keeps as
//! `actvar`, which name resolution and goto scoping share.
//!
//! Reports writes to read-only bindings, Lua 5.5 names no `global` declaration
//! covers, reads of locals a `continue` jumps over, and type functions reading
//! an outer local. Function boundaries keep the stack, since Lua also rejects
//! writes to a const upvalue.

use core::mem;

use luck_ast::{
    Attribute, AttributedName, Block, FunctionBody, FunctionDecl, GenericFor, NumericFor,
    Punctuated, RepeatLoop, Statement, Type, TypeDeclarationValue, Var, Visitor,
};
use luck_token::{LuaVersion, Span, Token};

use super::continue_until::skipped_local_count;
use super::gotos::{BlockStart, FunctionGotos, GotoTracker};
use crate::ParseError;

pub(super) fn check(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    ScopeChecker::run(block, version, false)
}

/// Report type functions that reference an outer local, a check Luau's parser
/// makes as it reads the body. `parse` runs this because it tracks no locals.
pub(crate) fn check_type_functions(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    ScopeChecker::run(block, version, true)
}

struct Binding<'ast> {
    /// `None` for a Lua 5.5 `global *`, which covers any name no other entry
    /// declares.
    name: Option<&'ast str>,
    is_global: bool,
    is_readonly: bool,
    /// The `until` condition's span while a `continue` jumps over this repeat
    /// body local. Cleared on report, so each local reports once.
    skipped_until: Option<Span>,
}

enum Resolved {
    Binding(usize),
    ImplicitGlobal,
    /// A Lua 5.5 name that no longer falls under the implicit `global *`.
    Undeclared,
}

struct ScopeChecker<'ast> {
    version: LuaVersion,
    /// Whether this walk only reports type functions reading an outer local,
    /// which is what `parse` runs. Every other rule waits for `validate`.
    is_type_function_pass: bool,
    bindings: Vec<Binding<'ast>>,
    /// Bindings below this index lie outside the innermost type function.
    type_function_base: usize,
    has_skipped_locals: bool,
    gotos: GotoTracker<'ast>,
    errors: Vec<ParseError>,
}

fn is_readonly_attribute(attribute: &Attribute) -> bool {
    matches!(attribute.name.identifier(), Some("const" | "close"))
}

impl<'ast> ScopeChecker<'ast> {
    fn run(
        block: &'ast Block,
        version: LuaVersion,
        is_type_function_pass: bool,
    ) -> Vec<ParseError> {
        let mut checker = Self {
            version,
            is_type_function_pass,
            bindings: Vec::new(),
            type_function_base: 0,
            has_skipped_locals: false,
            gotos: GotoTracker::new(version),
            errors: Vec::new(),
        };
        // Lua 5.2+: the chunk's `_ENV` upvalue, visible under any `global`.
        if version.has_env_upvalue() {
            checker.push_binding(Some("_ENV"), false, false);
        }
        checker.visit_block(block);
        checker.gotos.leave_function(FunctionGotos::default());
        let mut errors = checker.errors;
        errors.extend(checker.gotos.into_errors());
        errors
    }

    fn declare_local(&mut self, token: &'ast Token, is_readonly: bool) {
        if let Some(name) = token.identifier() {
            self.push_binding(Some(name), false, is_readonly);
        }
    }

    fn push_binding(&mut self, name: Option<&'ast str>, is_global: bool, is_readonly: bool) {
        self.bindings.push(Binding { name, is_global, is_readonly, skipped_until: None });
    }

    fn declare_attributed_names(
        &mut self,
        names: &'ast Punctuated<AttributedName>,
        is_const_stmt: bool,
        is_global: bool,
    ) {
        for attributed in names.iter() {
            let is_readonly =
                is_const_stmt || attributed.attribute.as_deref().is_some_and(is_readonly_attribute);
            if let Some(name) = attributed.name.identifier() {
                self.push_binding(Some(name), is_global, is_readonly);
            }
        }
    }

    /// Resolve `name` as `searchvar` and `buildvar` do: the innermost entry
    /// with this name, then the innermost `global *`, and a name matching
    /// neither is undeclared once some `global` declaration is active.
    fn resolve(&self, name: &str) -> Resolved {
        let mut collective = None;
        let mut has_named_global = false;
        for (i, binding) in self.bindings.iter().enumerate().rev() {
            match binding.name {
                Some(declared) if declared == name => return Resolved::Binding(i),
                Some(_) => has_named_global |= binding.is_global,
                None => collective = collective.or(Some(i)),
            }
        }
        match collective {
            Some(index) => Resolved::Binding(index),
            None if has_named_global => Resolved::Undeclared,
            None => Resolved::ImplicitGlobal,
        }
    }

    /// Whether a read can break a rule, sparing the resolve on every other
    /// read.
    const fn checks_reads(&self) -> bool {
        self.version.has_global() || self.type_function_base > 0 || self.has_skipped_locals
    }

    fn check_name(&mut self, token: &Token, is_write: bool) {
        let Some(name) = token.identifier() else {
            return;
        };
        let index = match self.resolve(name) {
            Resolved::ImplicitGlobal => return,
            Resolved::Undeclared => {
                self.errors.push(ParseError {
                    span: token.span,
                    message: format!("variable `{name}` not declared"),
                });
                return;
            }
            Resolved::Binding(index) => index,
        };
        if index < self.type_function_base {
            self.errors.push(ParseError {
                span: token.span,
                message: format!("type function cannot reference outer local `{name}`"),
            });
        }
        let Some(binding) = self.bindings.get_mut(index) else {
            return;
        };
        if let Some(condition_span) = binding.skipped_until.take() {
            self.errors.push(ParseError {
                span: condition_span,
                message: format!(
                    "local `{name}` used in the `until` condition is declared after a `continue` statement"
                ),
            });
        }
        if is_write && binding.is_readonly && !self.is_type_function_pass {
            self.errors.push(ParseError {
                span: token.span,
                message: format!("attempt to assign to const variable `{name}`"),
            });
        }
    }

    fn check_target(&mut self, target: &'ast Var) {
        match target {
            Var::Name(token) => self.check_name(token, true),
            // `t.x = 1` and `t[k] = 1` mutate the table, not the binding, and
            // const doesn't freeze contents.
            Var::FieldAccess(_) | Var::Index(_) => self.walk_var(target),
        }
    }

    fn open_block(&mut self) -> BlockStart {
        self.gotos.open_block(self.bindings.len())
    }

    fn close_block(&mut self, start: BlockStart) {
        self.bindings.truncate(start.active_vars());
        self.gotos.close_block(start);
    }

    /// Walk `block`'s statements in the current scope. `outer_active_vars` is
    /// the variable count outside the block, which is where a label at its
    /// end resolves, as if its locals had already closed.
    fn walk_statements(
        &mut self,
        block: &'ast Block,
        is_repeat_body: bool,
        outer_active_vars: usize,
    ) {
        // `block_follow(ls, 0)`: a label ends its block only if nothing but
        // labels and `;` follow, not `return`, and not a repeat's `until`.
        let trailing_start = if is_repeat_body || block.last_stmt.is_some() {
            block.stmts.len()
        } else {
            block
                .stmts
                .iter()
                .rposition(|stmt| {
                    !matches!(stmt, Statement::Label(_) | Statement::EmptyStatement(_))
                })
                .map_or(0, |i| i.saturating_add(1))
        };
        for (i, stmt) in block.stmts.iter().enumerate() {
            if let Statement::Label(label) = stmt {
                let active_vars =
                    if i >= trailing_start { outer_active_vars } else { self.bindings.len() };
                self.gotos.add_label(label, active_vars);
            } else if let Statement::Goto(goto) = stmt {
                self.gotos.add_goto(goto, self.bindings.len());
            } else {
                self.visit_stmt(stmt);
            }
        }
        if let Some(last) = &block.last_stmt {
            self.visit_last_stmt(last);
        }
    }

    /// The `until` condition sits inside the body's scope, so the body's
    /// locals stay active for it.
    fn visit_repeat_loop(&mut self, repeat_loop: &'ast RepeatLoop) {
        let start = self.open_block();
        self.walk_statements(&repeat_loop.block, true, start.active_vars());
        let skipped_count =
            if self.is_type_function_pass { 0 } else { skipped_local_count(&repeat_loop.block) };
        let first_skipped = self.bindings.len().saturating_sub(skipped_count);
        let condition_span = repeat_loop.condition.span();
        for binding in self.bindings.iter_mut().skip(first_skipped) {
            binding.skipped_until = Some(condition_span);
        }
        let had_skipped_locals = self.has_skipped_locals;
        self.has_skipped_locals |= skipped_count > 0;
        self.visit_expr(&repeat_loop.condition);
        self.has_skipped_locals = had_skipped_locals;
        self.close_block(start);
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor) {
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        let start = self.open_block();
        // Lua 5.5 makes the control variable read-only.
        self.declare_local(&numeric_for.name, self.version.has_const_for_variables());
        self.walk_statements(&numeric_for.block, false, start.active_vars());
        self.close_block(start);
    }

    fn visit_generic_for(&mut self, generic_for: &'ast GenericFor) {
        for binding in generic_for.names.iter() {
            if let Some(annotation) = &binding.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in generic_for.exprs.iter() {
            self.visit_expr(expr);
        }
        let start = self.open_block();
        for (i, binding) in generic_for.names.iter().enumerate() {
            // Lua 5.5: only the first variable is the control variable.
            self.declare_local(&binding.name, i == 0 && self.version.has_const_for_variables());
        }
        self.walk_statements(&generic_for.block, false, start.active_vars());
        self.close_block(start);
    }

    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl) {
        let name = &function_decl.name;
        // `function a()` writes the variable; `function a.b.c()` reads it and
        // writes a field.
        let is_plain_name = name.names.len() == 1 && name.method.is_none();
        if let Some(first) = name.names.first()
            && (is_plain_name || self.checks_reads())
        {
            self.check_name(first, is_plain_name);
        }
        let bindings_base = self.bindings.len();
        if name.method.is_some() {
            self.push_binding(Some("self"), false, false);
        }
        self.visit_function_body(&function_decl.body);
        self.bindings.truncate(bindings_base);
    }
}

impl<'ast> Visitor<'ast> for ScopeChecker<'ast> {
    fn visit_block(&mut self, block: &'ast Block) {
        let start = self.open_block();
        self.walk_statements(block, false, start.active_vars());
        self.close_block(start);
    }

    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for v in assignment.values.iter() {
                    self.visit_expr(v);
                }
                for target in assignment.targets.iter() {
                    self.check_target(target);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound_assignment) => {
                self.visit_expr(&compound_assignment.expr);
                self.check_target(&compound_assignment.var);
            }
            Statement::RepeatLoop(repeat_loop) => self.visit_repeat_loop(repeat_loop),
            Statement::NumericFor(numeric_for) => self.visit_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.visit_generic_for(generic_for),
            Statement::FunctionDecl(function_decl) => self.visit_function_decl(function_decl),
            Statement::LocalFunction(local_function) => {
                self.declare_local(&local_function.name, local_function.is_const);
                self.visit_function_body(&local_function.body);
            }
            Statement::LocalAssignment(local) => {
                self.walk_attributed_names(&local.names);
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
                self.declare_attributed_names(&local.names, local.is_const, false);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global) => {
                if let Some(exprs) = &global.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
                self.declare_attributed_names(&global.names, false, true);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                if let Some(name) = global_function.name.identifier() {
                    self.push_binding(Some(name), true, false);
                }
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5
            Statement::GlobalStar(global_star) => {
                let is_readonly = global_star.attribute.as_ref().is_some_and(is_readonly_attribute);
                self.push_binding(None, true, is_readonly);
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                if matches!(type_declaration.type_value, TypeDeclarationValue::TypeFunction(_)) {
                    let enclosing = mem::replace(&mut self.type_function_base, self.bindings.len());
                    self.walk_stmt(stmt);
                    self.type_function_base = enclosing;
                } else {
                    self.walk_stmt(stmt);
                }
            }
            Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::IfStatement(_)
            | Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::Break(_)
            | Statement::Error(_) => self.walk_stmt(stmt),
        }
    }

    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        let bindings_base = self.bindings.len();
        for param in body.params.iter() {
            self.declare_local(&param.name, false);
        }
        // 5.5 manual: a vararg's name "denotes a read-only local variable".
        if let Some(vararg) = &body.vararg
            && let Some(name) = &vararg.name
        {
            self.declare_local(name, true);
        }
        let enclosing_gotos = self.gotos.enter_function();
        self.walk_function_body(body);
        self.gotos.leave_function(enclosing_gotos);
        self.bindings.truncate(bindings_base);
    }

    fn visit_var(&mut self, var: &'ast Var) {
        if let Var::Name(token) = var
            && self.checks_reads()
        {
            self.check_name(token, false);
        }
        self.walk_var(var);
    }

    // Types generate no code, so outside the type-function pass a `typeof`
    // doesn't read or write anything.
    fn visit_type(&mut self, type_value: &'ast Type) {
        if self.is_type_function_pass {
            self.walk_type(type_value);
        }
    }
}
