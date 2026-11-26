use luck_ast::{
    Assignment, Block, CompoundAssignment, ConditionalBinding, DeclaredExternType,
    DeclaredFunction, DoBlock, Expression, ExternTypeMember, FunctionAttribute, FunctionBody,
    FunctionDecl, GenericFor, GenericTypeList, IfStatement, LocalAssignment, LocalAssignmentKind,
    LocalFunction, LocalFunctionKind, NumericFor, RepeatLoop, Slot, Statement, Type,
    TypeDeclaration, TypeDeclarationValue, TypedName, VarArgParam, WhileLoop,
};
use luck_token::CompoundOp;

use super::Printer;

impl Printer {
    pub(super) fn print_block(&mut self, block: &Block<'_>) {
        let mut previous = None;
        for stmt in &block.stmts {
            if starts_with_paren(stmt) && previous.is_some_and(is_open) {
                self.piece(";");
            }
            self.print_stmt(stmt);
            previous = Some(stmt);
        }
    }

    #[expect(
        clippy::inline_always,
        reason = "measured: folded into `print_block`, it prints up to 3% faster"
    )]
    #[inline(always)]
    fn print_stmt(&mut self, stmt: &Statement<'_>) {
        match stmt {
            Statement::Assignment(assignment) => self.print_assignment(assignment),
            Statement::FunctionCall(call) => self.print_suffixed(call),
            Statement::DoBlock(do_block) => self.print_do_block(do_block),
            Statement::WhileLoop(while_loop) => self.print_while_loop(while_loop),
            Statement::RepeatLoop(repeat_loop) => self.print_repeat_loop(repeat_loop),
            Statement::IfStatement(if_stmt) => self.print_if_stmt(if_stmt),
            Statement::NumericFor(numeric_for) => self.print_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.print_generic_for(generic_for),
            Statement::FunctionDecl(declaration) => self.print_function_decl(declaration),
            Statement::LocalFunction(local_function) => self.print_local_function(local_function),
            Statement::LocalAssignment(local) => self.print_local_assignment(local),
            Statement::Return(return_stmt) => self.print_return(&return_stmt.exprs),
            Statement::Break(_) => self.piece("break"),
            Statement::Continue(_) => self.piece("continue"),
            Statement::CompoundAssignment(compound) => self.print_compound_assignment(compound),
            Statement::TypeDeclaration(declaration) => self.print_type_declaration(declaration),
            Statement::DeclaredGlobal(global) => {
                self.piece("declare");
                self.name(global.name.text.as_str());
                self.piece(":");
                self.print_type(&global.ty);
            }
            Statement::DeclaredFunction(function) => {
                self.print_declared_function(function, "declare function");
            }
            Statement::DeclaredExternType(extern_type) => {
                self.print_declared_extern_type(extern_type);
            }
            Statement::Error(_) => {}
        }
    }

    /// The attributes, then `keywords`, or the chain an attributed `declare`
    /// opens and `function`, then the name and the signature.
    fn print_declared_function(&mut self, function: &DeclaredFunction<'_>, keywords: &str) {
        if !function.attributes.is_empty() {
            self.print_function_attributes(&function.attributes);
        }
        if let Some(chain) = &function.declare_chain {
            self.print_suffixed(chain);
            self.piece("function");
        } else {
            self.piece(keywords);
        }
        self.name(function.name.text.as_str());
        self.print_signature(
            function.generics.as_deref(),
            &function.params,
            function.vararg.as_deref(),
            function.return_type.as_deref(),
        );
    }

    /// The members print one after another, as Luau reads them.
    fn print_declared_extern_type(&mut self, extern_type: &DeclaredExternType<'_>) {
        self.piece("declare extern type");
        self.name(extern_type.name.text.as_str());
        if let Some(supertype) = &extern_type.supertype {
            self.piece("extends");
            self.name(supertype.text.as_str());
        }
        self.piece("with");
        for member in &extern_type.members {
            match member {
                ExternTypeMember::Field(field) => self.print_type_field(field),
                ExternTypeMember::Method(method) => {
                    self.print_declared_function(method, "function");
                }
            }
        }
        self.piece("end");
    }

    fn print_assignment(&mut self, assignment: &Assignment<'_>) {
        self.list(&assignment.targets, Self::print_var);
        self.piece("=");
        self.list(&assignment.values, Self::print_free_expr);
    }

    fn print_do_block(&mut self, do_block: &DoBlock<'_>) {
        self.piece("do");
        self.print_block(&do_block.block);
        self.piece("end");
    }

    fn print_while_loop(&mut self, while_loop: &WhileLoop<'_>) {
        self.piece("while");
        self.print_condition(while_loop.binding.as_deref(), &while_loop.condition);
        self.piece("do");
        self.print_block(&while_loop.block);
        self.piece("end");
    }

    fn print_repeat_loop(&mut self, repeat_loop: &RepeatLoop<'_>) {
        self.piece("repeat");
        self.print_block(&repeat_loop.block);
        self.piece("until");
        self.print_free_expr(&repeat_loop.condition);
    }

    fn print_if_stmt(&mut self, if_stmt: &IfStatement<'_>) {
        self.piece("if");
        self.print_condition(if_stmt.binding.as_deref(), &if_stmt.condition);
        self.piece("then");
        self.print_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.piece("elseif");
            self.print_condition(clause.binding.as_deref(), &clause.condition);
            self.piece("then");
            self.print_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.piece("else");
            self.print_block(&else_clause.block);
        }
        self.piece("end");
    }

    fn print_numeric_for(&mut self, numeric_for: &NumericFor<'_>) {
        self.piece("for");
        self.print_typed_name(&numeric_for.variable);
        self.piece("=");
        self.print_free_expr(&numeric_for.start);
        self.piece(",");
        self.print_free_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.piece(",");
            self.print_free_expr(step);
        }
        self.piece("do");
        self.print_block(&numeric_for.block);
        self.piece("end");
    }

    fn print_generic_for(&mut self, generic_for: &GenericFor<'_>) {
        self.piece("for");
        self.list(&generic_for.names, Self::print_typed_name);
        self.piece("in");
        self.list(&generic_for.exprs, Self::print_free_expr);
        self.piece("do");
        self.print_block(&generic_for.block);
        self.piece("end");
    }

    fn print_function_decl(&mut self, declaration: &FunctionDecl<'_>) {
        if !declaration.attributes.is_empty() {
            self.print_function_attributes(&declaration.attributes);
        }
        self.piece("function");
        for (index, name) in declaration.name.names.iter().enumerate() {
            if index != 0 {
                self.piece(".");
            }
            self.name(name.text.as_str());
        }
        if let Some(method) = &declaration.name.method {
            self.piece(":");
            self.name(method.text.as_str());
        }
        self.print_function_body(&declaration.body);
    }

    fn print_local_function(&mut self, local_function: &LocalFunction<'_>) {
        if !local_function.attributes.is_empty() {
            self.print_function_attributes(&local_function.attributes);
        }
        match local_function.kind {
            LocalFunctionKind::Local => self.piece("local function"),
            LocalFunctionKind::Const => self.piece("const function"),
            LocalFunctionKind::Export => self.piece("export function"),
        }
        self.name(local_function.name.text.as_str());
        self.print_function_body(&local_function.body);
    }

    fn print_local_assignment(&mut self, local: &LocalAssignment<'_>) {
        match local.kind {
            LocalAssignmentKind::Local => self.piece("local"),
            LocalAssignmentKind::Const => self.piece("const"),
            LocalAssignmentKind::ExportLocal => self.piece("export local"),
            LocalAssignmentKind::ExportConst => self.piece("export const"),
        }
        self.list(&local.names, Self::print_typed_name);
        if !local.exprs.is_empty() {
            self.piece("=");
            self.list(&local.exprs, Self::print_free_expr);
        }
    }

    fn print_return(&mut self, exprs: &[Expression<'_>]) {
        self.piece("return");
        self.list(exprs, Self::print_free_expr);
    }

    /// Each operator prints as a constant piece, as a binary one does.
    fn print_compound_assignment(&mut self, compound: &CompoundAssignment<'_>) {
        self.print_var(&compound.var);
        match compound.op {
            CompoundOp::Add => self.piece("+="),
            CompoundOp::Sub => self.piece("-="),
            CompoundOp::Mul => self.piece("*="),
            CompoundOp::Div => self.piece("/="),
            CompoundOp::FloorDiv => self.piece("//="),
            CompoundOp::Mod => self.piece("%="),
            CompoundOp::Pow => self.piece("^="),
            CompoundOp::Concat => self.piece("..="),
        }
        self.print_free_expr(&compound.expr);
    }

    fn print_type_declaration(&mut self, declaration: &TypeDeclaration<'_>) {
        if declaration.is_exported {
            self.piece("export");
        }
        self.piece("type");
        if matches!(declaration.value, TypeDeclarationValue::TypeFunction(_)) {
            self.piece("function");
        }
        self.name(declaration.name.text.as_str());
        if let Some(generics) = &declaration.generics {
            self.print_generic_type_list(generics);
        }
        match &declaration.value {
            TypeDeclarationValue::Alias(ty) => {
                self.piece("=");
                self.print_type(ty);
            }
            TypeDeclarationValue::TypeFunction(body) => self.print_function_body(body),
        }
    }

    pub(super) fn print_condition(
        &mut self,
        binding: Option<&ConditionalBinding<'_>>,
        condition: &Expression<'_>,
    ) {
        if let Some(binding) = binding {
            self.piece(if binding.is_const { "const" } else { "local" });
            self.print_typed_name(&binding.name);
            self.piece("=");
        }
        self.print_free_expr(condition);
    }

    fn print_typed_name(&mut self, typed_name: &TypedName<'_>) {
        self.name(typed_name.name.text.as_str());
        if let Some(ty) = &typed_name.type_annotation {
            self.piece(":");
            self.print_type(ty);
        }
    }

    /// Only the bracketed form takes arguments, and a lone table or string
    /// needs no parentheses there, as a call's doesn't.
    pub(super) fn print_function_attributes(&mut self, attributes: &[FunctionAttribute<'_>]) {
        for attribute in attributes {
            if let Some(args) = &attribute.args {
                self.piece("@[");
                self.name(attribute.name.text.as_str());
                if let [arg @ (Expression::TableConstructor(_) | Expression::StringLiteral(_))] =
                    &**args
                {
                    self.print_free_expr(arg);
                } else {
                    self.piece("(");
                    self.list(args, Self::print_free_expr);
                    self.piece(")");
                }
                self.piece("]");
            } else {
                self.piece("@");
                self.name(attribute.name.text.as_str());
            }
        }
    }

    pub(super) fn print_function_body(&mut self, body: &FunctionBody<'_>) {
        self.print_signature(
            body.generics.as_deref(),
            &body.params,
            body.vararg.as_deref(),
            body.return_type.as_deref(),
        );
        self.print_block(&body.block);
        self.piece("end");
    }

    fn print_signature(
        &mut self,
        generics: Option<&GenericTypeList<'_>>,
        params: &[TypedName<'_>],
        vararg: Option<&VarArgParam<'_>>,
        return_type: Option<&Type<'_>>,
    ) {
        if let Some(generics) = generics {
            self.print_generic_type_list(generics);
        }
        self.piece("(");
        self.list(params, Self::print_typed_name);
        if let Some(vararg) = vararg {
            if !params.is_empty() {
                self.piece(",");
            }
            self.piece("...");
            if let Some(ty) = &vararg.type_annotation {
                self.piece(":");
                self.print_type(ty);
            }
        }
        self.piece(")");
        if let Some(return_type) = return_type {
            self.piece(":");
            self.print_type(return_type);
        }
    }
}

/// A name, as most prefixes are, never takes parentheses.
fn starts_with_paren(stmt: &Statement<'_>) -> bool {
    stmt.leading_prefix().is_some_and(|prefix| {
        !matches!(prefix, Expression::Name(_))
            && (matches!(prefix, Expression::Parenthesized(_)) || prefix.needs_parens(Slot::PREFIX))
    })
}

/// Whether `stmt` can end in an expression or a type, which a `(` printed
/// right after it would call. A bare `return` would take it as its value.
fn is_open(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::RepeatLoop(_)
        | Statement::LocalAssignment(_)
        | Statement::Return(_)
        | Statement::CompoundAssignment(_)
        | Statement::DeclaredGlobal(_)
        // Prints nothing, so whatever it follows ends the output.
        | Statement::Error(_) => true,
        Statement::TypeDeclaration(declaration) => {
            matches!(declaration.value, TypeDeclarationValue::Alias(_))
        }
        Statement::DeclaredFunction(function) => function.return_type.is_some(),
        Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::DeclaredExternType(_) => false,
    }
}
