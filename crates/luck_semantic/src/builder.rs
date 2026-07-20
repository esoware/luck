use compact_str::CompactString;
use luck_ast::Expression;
use luck_ast::expr::*;
use luck_ast::shared::*;
use luck_ast::visitor::Visitor;
use luck_token::TokenKind;

use crate::scope::*;

/// Builds a ScopeTree by walking the AST.
pub struct ScopeTreeBuilder {
    pub tree: ScopeTree,
    // NonEmptyStack layout: the top lives in `current_scope` so reading it
    // is branchless; `outer_scopes` holds only the enclosing scopes.
    current_scope: ScopeId,
    outer_scopes: Vec<ScopeId>,
    /// One scoped map for the whole build: per name, the stack of live
    /// declarations (innermost last). Sequential shadowing is stack order;
    /// resolution is one O(1) lookup. A flat scan-backwards `actives` array
    /// benchmarked quadratic on files with hundreds of live module-level
    /// locals, and per-scope maps cost an allocation per block - this keeps
    /// one map whose per-name stacks (and their capacity) are reused.
    bindings: std::collections::HashMap<CompactString, Vec<SymbolId>>,
    /// Undo log: names declared since each scope entry, popped on exit.
    declared_names: Vec<CompactString>,
    /// `declared_names` length at each scope entry.
    binding_marks: Vec<usize>,
}

impl ScopeTreeBuilder {
    pub fn new() -> Self {
        Self {
            tree: ScopeTree::new(),
            // Placeholder until build() adds the module scope, which is
            // always the first scope and therefore index 0.
            current_scope: ScopeId::from_index(0),
            outer_scopes: Vec::new(),
            bindings: std::collections::HashMap::new(),
            declared_names: Vec::new(),
            binding_marks: Vec::new(),
        }
    }

    pub fn build(mut self, block: &Block) -> ScopeTree {
        self.current_scope = self.tree.add_scope(None, ScopeKind::Module, block.span);
        self.visit_block(block);
        self.tree
    }

    fn push_scope(&mut self, kind: ScopeKind, span: luck_token::Span) -> ScopeId {
        let id = self.tree.add_scope(Some(self.current_scope), kind, span);
        self.outer_scopes
            .push(std::mem::replace(&mut self.current_scope, id));
        self.binding_marks.push(self.declared_names.len());
        id
    }

    fn pop_scope(&mut self) {
        self.current_scope = self
            .outer_scopes
            .pop()
            .expect("pop_scope without matching push");
        let mark = self
            .binding_marks
            .pop()
            .expect("pop_scope without matching push");
        // Emptied stacks stay in the map so a re-declared name reuses the
        // Vec's capacity instead of reallocating.
        for name in self.declared_names.drain(mark..) {
            if let Some(stack) = self.bindings.get_mut(&name) {
                stack.pop();
            }
        }
    }

    fn resolve(&self, name: &str) -> Option<SymbolId> {
        self.bindings
            .get(name)
            .and_then(|stack| stack.last())
            .copied()
    }

    fn declare_local(
        &mut self,
        name: &CompactString,
        span: luck_token::Span,
        kind: SymbolKind,
    ) -> SymbolId {
        let shadows = self.resolve(name);
        let id = self
            .tree
            .add_symbol(name.clone(), self.current_scope, kind, span, shadows);
        self.bindings.entry(name.clone()).or_default().push(id);
        self.declared_names.push(name.clone());
        id
    }

    fn reference_name(
        &mut self,
        name: &CompactString,
        span: luck_token::Span,
        kind: ReferenceKind,
    ) {
        let resolved = self.resolve(name);
        self.tree
            .add_reference(name.clone(), span, self.current_scope, kind, resolved);
    }

    fn visit_var_read(&mut self, var: &Var) {
        match var {
            Var::Name(token) => {
                if let TokenKind::Identifier(name) = &token.kind {
                    self.reference_name(name, token.span, ReferenceKind::Read);
                }
            }
            Var::Index(idx) => {
                self.visit_expression(&idx.prefix);
                self.visit_expression(&idx.index);
            }
            Var::FieldAccess(fa) => {
                self.visit_expression(&fa.prefix);
            }
        }
    }

    fn visit_var_write(&mut self, var: &Var) {
        match var {
            Var::Name(token) => {
                if let TokenKind::Identifier(name) = &token.kind {
                    self.reference_name(name, token.span, ReferenceKind::Write);
                }
            }
            Var::Index(idx) => {
                self.visit_expression(&idx.prefix);
                self.visit_expression(&idx.index);
            }
            Var::FieldAccess(fa) => {
                self.visit_expression(&fa.prefix);
            }
        }
    }

    fn visit_call(&mut self, call: &luck_ast::expr::FunctionCall) {
        self.visit_expression(&call.callee);
        match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for expr in args.iter() {
                    self.visit_expression(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            FunctionArgs::StringLiteral(_) => {}
        }
        // Method names are field selectors, not variable references.
    }

    fn visit_function_body(&mut self, body: &FunctionBody) {
        self.visit_function_body_with_method(body, false);
    }

    fn visit_function_body_with_method(&mut self, body: &FunctionBody, is_method: bool) {
        self.push_scope(ScopeKind::Function, body.span);

        if is_method {
            self.declare_local(
                &CompactString::const_new("self"),
                body.span,
                SymbolKind::Parameter,
            );
        }

        for param in body.params.iter() {
            if let TokenKind::Identifier(name) = &param.name.kind {
                self.declare_local(name, param.name.span, SymbolKind::Parameter);
            }
            if let Some(annotation) = &param.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(vararg) = &body.vararg {
            if let Some(annotation) = &vararg.type_annotation {
                self.visit_type(annotation);
            }
        }
        if let Some(annotation) = &body.return_type {
            self.visit_type(annotation);
        }

        self.visit_block(&body.block);
        self.pop_scope();
    }
}

impl<'ast> Visitor<'ast> for ScopeTreeBuilder {
    fn visit_block(&mut self, block: &'ast Block) {
        for stmt in &block.stmts {
            self.visit_statement(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.visit_last_statement(last);
        }
    }

    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        match stmt {
            luck_ast::Statement::LocalAssignment(local) => {
                // Visit values first (they see the outer scope)
                if let Some(exprs) = &local.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
                for attributed in local.names.iter() {
                    // Annotations can reference runtime bindings via
                    // typeof(expr).
                    if let Some(annotation) = &attributed.type_annotation {
                        self.visit_type(annotation);
                    }
                    if let TokenKind::Identifier(n) = &attributed.name.kind {
                        self.declare_local(n, attributed.name.span, SymbolKind::Local);
                    }
                }
            }
            luck_ast::Statement::Assignment(assign) => {
                for expr in assign.values.iter() {
                    self.visit_expression(expr);
                }
                for var in assign.targets.iter() {
                    self.visit_var_write(var);
                }
            }
            luck_ast::Statement::LocalFunction(func) => {
                // Declare the function name first (allows recursion)
                if let TokenKind::Identifier(name) = &func.name.kind {
                    self.declare_local(name, func.name.span, SymbolKind::FunctionName);
                }
                self.visit_function_body(&func.body);
            }
            luck_ast::Statement::FunctionDecl(decl) => {
                // `function f()` writes `f`, but `function t.m()` /
                // `function t:m()` READS `t` and writes only the field -
                // recording a write here made the canonical module pattern
                // (`local M = {} function M.f() end return M`) light up
                // unused/overwritten warnings on every real module.
                if let Some(first) = decl.name.names.first()
                    && let TokenKind::Identifier(name) = &first.kind
                {
                    let is_field_write = decl.name.names.len() > 1 || decl.name.method.is_some();
                    let kind = if is_field_write {
                        ReferenceKind::Read
                    } else {
                        ReferenceKind::Write
                    };
                    self.reference_name(name, first.span, kind);
                }
                let is_method = decl.name.method.is_some();
                self.visit_function_body_with_method(&decl.body, is_method);
            }
            luck_ast::Statement::FunctionCall(call) => {
                self.visit_call(&call.call);
            }
            luck_ast::Statement::DoBlock(do_block) => {
                self.push_scope(ScopeKind::Block, do_block.span);
                self.visit_block(&do_block.block);
                self.pop_scope();
            }
            luck_ast::Statement::WhileLoop(while_loop) => {
                self.visit_expression(&while_loop.condition);
                self.push_scope(ScopeKind::Loop, while_loop.span);
                self.visit_block(&while_loop.block);
                self.pop_scope();
            }
            luck_ast::Statement::RepeatLoop(repeat_loop) => {
                self.push_scope(ScopeKind::Loop, repeat_loop.span);
                self.visit_block(&repeat_loop.block);
                // Condition can see locals from the loop body
                self.visit_expression(&repeat_loop.condition);
                self.pop_scope();
            }
            luck_ast::Statement::IfStatement(if_stmt) => {
                self.visit_expression(&if_stmt.condition);
                self.push_scope(ScopeKind::Block, if_stmt.block.span);
                self.visit_block(&if_stmt.block);
                self.pop_scope();

                for clause in &if_stmt.elseif_clauses {
                    self.visit_expression(&clause.condition);
                    self.push_scope(ScopeKind::Block, clause.block.span);
                    self.visit_block(&clause.block);
                    self.pop_scope();
                }

                if let Some(else_clause) = &if_stmt.else_clause {
                    self.push_scope(ScopeKind::Block, else_clause.block.span);
                    self.visit_block(&else_clause.block);
                    self.pop_scope();
                }
            }
            luck_ast::Statement::NumericFor(num_for) => {
                self.visit_expression(&num_for.start);
                self.visit_expression(&num_for.limit);
                if let Some(step) = &num_for.step {
                    self.visit_expression(step);
                }
                self.push_scope(ScopeKind::Loop, num_for.span);
                if let TokenKind::Identifier(name) = &num_for.name.kind {
                    self.declare_local(name, num_for.name.span, SymbolKind::NumericForVariable);
                }
                if let Some(annotation) = &num_for.type_annotation {
                    self.visit_type(annotation);
                }
                self.visit_block(&num_for.block);
                self.pop_scope();
            }
            luck_ast::Statement::GenericFor(gen_for) => {
                // Visit iterators in outer scope
                for expr in gen_for.exprs.iter() {
                    self.visit_expression(expr);
                }
                self.push_scope(ScopeKind::Loop, gen_for.span);
                for binding in gen_for.names.iter() {
                    if let TokenKind::Identifier(n) = &binding.name.kind {
                        self.declare_local(n, binding.name.span, SymbolKind::IteratorVariable);
                    }
                    if let Some(annotation) = &binding.type_annotation {
                        self.visit_type(annotation);
                    }
                }
                self.visit_block(&gen_for.block);
                self.pop_scope();
            }
            luck_ast::Statement::CompoundAssignment(compound) => {
                self.visit_expression(&compound.expr);
                match &compound.var {
                    Var::Name(token) => {
                        if let TokenKind::Identifier(name) = &token.kind {
                            self.reference_name(name, token.span, ReferenceKind::ReadWrite);
                        }
                    }
                    _ => self.visit_var_write(&compound.var),
                }
            }
            // Lua 5.5 globals declare no locals, but their initializers
            // and function bodies still reference the enclosing scopes.
            luck_ast::Statement::GlobalDeclaration(global_decl) => {
                if let Some(exprs) = &global_decl.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
            }
            luck_ast::Statement::GlobalFunction(global_func) => {
                self.visit_function_body(&global_func.body);
            }
            luck_ast::Statement::GlobalStar(_) => {}
            // Type aliases reference runtime bindings via typeof(expr),
            // and `type function` bodies are ordinary Luau code.
            luck_ast::Statement::TypeDeclaration(type_decl) => {
                if let Some(generics) = &type_decl.generics {
                    self.walk_generic_type_list(generics);
                }
                match &type_decl.type_value {
                    luck_ast::stmt::TypeDeclarationValue::Alias(alias) => self.visit_type(alias),
                    luck_ast::stmt::TypeDeclarationValue::TypeFunction(body) => {
                        self.visit_function_body(body)
                    }
                }
            }
            luck_ast::Statement::Goto(_)
            | luck_ast::Statement::Label(_)
            | luck_ast::Statement::EmptyStatement(_)
            | luck_ast::Statement::Break(_)
            | luck_ast::Statement::Error(_) => {}
        }
    }

    fn visit_expression(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Var(var) => self.visit_var_read(var),
            Expression::FunctionCall(call) => self.visit_call(call),
            Expression::FunctionDef(func_def) => {
                self.visit_function_body(&func_def.body);
            }
            Expression::BinaryOp(binop) => {
                self.visit_expression(&binop.left);
                self.visit_expression(&binop.right);
            }
            Expression::UnaryOp(unop) => {
                self.visit_expression(&unop.operand);
            }
            Expression::Parenthesized(paren) => {
                self.visit_expression(&paren.expr);
            }
            Expression::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            Expression::IfExpression(if_expr) => {
                self.visit_expression(&if_expr.condition);
                self.visit_expression(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.visit_expression(&clause.condition);
                    self.visit_expression(&clause.expr);
                }
                self.visit_expression(&if_expr.else_expr);
            }
            Expression::InterpolatedString(interp) => {
                for segment in &interp.segments {
                    if let Some(expr) = &segment.expr {
                        self.visit_expression(expr);
                    }
                }
            }
            Expression::TypeCast(cast) => {
                self.visit_expression(&cast.expr);
                self.visit_type(&cast.type_annotation);
            }
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
        }
    }

    fn visit_last_statement(&mut self, stmt: &'ast luck_ast::LastStatement) {
        match stmt {
            luck_ast::LastStatement::Return(ret) => {
                for expr in ret.exprs.iter() {
                    self.visit_expression(expr);
                }
            }
            luck_ast::LastStatement::Break(_)
            | luck_ast::LastStatement::Continue(_)
            | luck_ast::LastStatement::Error(_) => {}
        }
    }
}

impl ScopeTreeBuilder {
    fn visit_table_constructor(&mut self, table: &TableConstructor) {
        for field in table.fields.iter() {
            match field {
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    self.visit_expression(value);
                }
                Field::Bracketed { key, value, .. } => {
                    self.visit_expression(key);
                    self.visit_expression(value);
                }
            }
        }
    }
}

impl Default for ScopeTreeBuilder {
    fn default() -> Self {
        Self::new()
    }
}
