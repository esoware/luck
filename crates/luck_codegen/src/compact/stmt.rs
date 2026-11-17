use luck_ast::{
    Attribute, AttributedName, Block, Expression, FunctionAttribute, FunctionBody, LastStatement,
    LocalFunctionKind, Parameter, Statement, TypeDeclarationValue, stmt_starts_with_paren,
};

use super::Printer;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the printer is one type with its node families split by file"
)]
impl Printer {
    pub(super) fn print_block(&mut self, block: &Block<'_>) {
        let mut previous = None;
        for stmt in &block.stmts {
            if stmt_starts_with_paren(stmt) && previous.is_some_and(is_open) {
                self.piece(";");
            }
            self.print_stmt(stmt);
            previous = Some(stmt);
        }
        if let Some(last) = &block.last_stmt {
            match last {
                LastStatement::Return(return_stmt) => {
                    self.piece("return");
                    self.list(&return_stmt.exprs, ",", Self::print_expr);
                }
                LastStatement::Break(_) => self.piece("break"),
                // Luau
                LastStatement::Continue(_) => self.piece("continue"),
                LastStatement::Error(_) => {}
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per kind of statement")]
    #[expect(
        clippy::inline_always,
        reason = "folded into `print_block`, a nested block costs one frame, and it measures faster"
    )]
    #[inline(always)]
    fn print_stmt(&mut self, stmt: &Statement<'_>) {
        match stmt {
            Statement::Assignment(assignment) => {
                self.list(&assignment.targets, ",", Self::print_var);
                self.piece("=");
                self.list(&assignment.values, ",", Self::print_expr);
            }
            Statement::FunctionCall(call) => self.print_suffixed(call),
            Statement::DoBlock(do_block) => {
                self.piece("do");
                self.print_block(&do_block.block);
                self.piece("end");
            }
            Statement::WhileLoop(while_loop) => {
                self.piece("while");
                self.print_expr(&while_loop.condition);
                self.piece("do");
                self.print_block(&while_loop.block);
                self.piece("end");
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.piece("repeat");
                self.print_block(&repeat_loop.block);
                self.piece("until");
                self.print_expr(&repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                self.piece("if");
                self.print_expr(&if_stmt.condition);
                self.piece("then");
                self.print_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.piece("elseif");
                    self.print_expr(&clause.condition);
                    self.piece("then");
                    self.print_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.piece("else");
                    self.print_block(&else_clause.block);
                }
                self.piece("end");
            }
            Statement::NumericFor(numeric_for) => {
                self.piece("for");
                self.name(numeric_for.name.text);
                if let Some(ty) = &numeric_for.type_annotation {
                    self.piece(":");
                    self.print_type(ty);
                }
                self.piece("=");
                self.print_expr(&numeric_for.start);
                self.piece(",");
                self.print_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.piece(",");
                    self.print_expr(step);
                }
                self.piece("do");
                self.print_block(&numeric_for.block);
                self.piece("end");
            }
            Statement::GenericFor(generic_for) => {
                self.piece("for");
                self.list(&generic_for.names, ",", Self::print_param);
                self.piece("in");
                self.list(&generic_for.exprs, ",", Self::print_expr);
                self.piece("do");
                self.print_block(&generic_for.block);
                self.piece("end");
            }
            Statement::FunctionDecl(declaration) => {
                self.print_function_attributes(&declaration.attributes);
                self.piece("function");
                self.list(&declaration.name.names, ".", |printer, name| printer.name(name.text));
                if let Some(method) = &declaration.name.method {
                    self.piece(":");
                    self.name(method.text);
                }
                self.print_function_body(&declaration.body);
            }
            Statement::LocalFunction(local_function) => {
                self.print_function_attributes(&local_function.attributes);
                match local_function.kind {
                    LocalFunctionKind::Local => self.piece("local function"),
                    LocalFunctionKind::Const => self.piece("const function"),
                    LocalFunctionKind::Export => self.piece("export function"),
                }
                self.name(local_function.name.text);
                self.print_function_body(&local_function.body);
            }
            Statement::LocalAssignment(local) => {
                if local.kind.is_exported() {
                    self.piece("export");
                }
                if local.kind.is_const() {
                    self.piece("const");
                } else {
                    self.piece("local");
                }
                self.list(&local.names, ",", Self::print_attributed_name);
                self.print_values(&local.exprs);
            }
            Statement::EmptyStatement(_) => self.piece(";"),
            // Lua 5.2+
            Statement::Goto(goto) => {
                self.piece("goto");
                self.name(goto.name.text);
            }
            // Lua 5.2+
            Statement::Label(label) => {
                self.piece("::");
                self.name(label.name.text);
                self.piece("::");
            }
            // Lua 5.5
            Statement::GlobalDeclaration(declaration) => {
                self.piece("global");
                self.list(&declaration.names, ",", Self::print_attributed_name);
                self.print_values(&declaration.exprs);
            }
            // Lua 5.5
            Statement::GlobalFunction(function) => {
                self.piece("global function");
                self.name(function.name.text);
                self.print_function_body(&function.body);
            }
            // Lua 5.5
            Statement::GlobalStar(global_star) => {
                self.piece("global");
                if let Some(attribute) = &global_star.attribute {
                    self.print_attribute(attribute);
                }
                self.piece("*");
            }
            // Lua 5.2+
            Statement::Break(_) => self.piece("break"),
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.print_var(&compound.var);
                self.piece(compound.op.static_text());
                self.print_expr(&compound.expr);
            }
            // Luau
            Statement::TypeDeclaration(declaration) => {
                if declaration.is_exported {
                    self.piece("export");
                }
                self.piece("type");
                if matches!(declaration.value, TypeDeclarationValue::TypeFunction(_)) {
                    self.piece("function");
                }
                self.name(declaration.name.text);
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
            Statement::Error(_) => {}
        }
    }

    fn print_values(&mut self, exprs: &[Expression<'_>]) {
        if !exprs.is_empty() {
            self.piece("=");
            self.list(exprs, ",", Self::print_expr);
        }
    }

    fn print_attributed_name(&mut self, attributed: &AttributedName<'_>) {
        self.name(attributed.name.text);
        if let Some(ty) = &attributed.type_annotation {
            self.piece(":");
            self.print_type(ty);
        }
        if let Some(attribute) = &attributed.attribute {
            self.print_attribute(attribute);
        }
    }

    fn print_attribute(&mut self, attribute: &Attribute<'_>) {
        self.piece("<");
        self.name(attribute.name.text);
        self.piece(">");
    }

    /// Luau: only the bracketed form takes arguments, and a lone table or
    /// string needs no parentheses there, as a call's doesn't.
    pub(super) fn print_function_attributes(&mut self, attributes: &[FunctionAttribute<'_>]) {
        for attribute in attributes {
            if let Some(args) = &attribute.args {
                self.piece("@[");
                self.name(attribute.name.text);
                if let [arg @ (Expression::TableConstructor(_) | Expression::StringLiteral(_))] =
                    &**args
                {
                    self.print_expr(arg);
                } else {
                    self.piece("(");
                    self.list(args, ",", Self::print_expr);
                    self.piece(")");
                }
                self.piece("]");
            } else {
                self.piece("@");
                self.name(attribute.name.text);
            }
        }
    }

    pub(super) fn print_function_body(&mut self, body: &FunctionBody<'_>) {
        if let Some(generics) = &body.generics {
            self.print_generic_type_list(generics);
        }
        self.piece("(");
        self.list(&body.params, ",", Self::print_param);
        if let Some(vararg) = &body.vararg {
            if !body.params.is_empty() {
                self.piece(",");
            }
            self.piece("...");
            if let Some(name) = &vararg.name {
                self.name(name.text);
            }
            if let Some(ty) = &vararg.type_annotation {
                self.piece(":");
                self.print_type(ty);
            }
        }
        self.piece(")");
        if let Some(return_type) = &body.return_type {
            self.piece(":");
            self.print_type(return_type);
        }
        self.print_block(&body.block);
        self.piece("end");
    }

    fn print_param(&mut self, param: &Parameter<'_>) {
        self.name(param.name.text);
        if let Some(ty) = &param.type_annotation {
            self.piece(":");
            self.print_type(ty);
        }
    }
}

/// Whether `stmt` can end in an expression or a type, which a `(` printed
/// right after it would call.
fn is_open(stmt: &Statement<'_>) -> bool {
    match stmt {
        Statement::Assignment(_)
        | Statement::FunctionCall(_)
        | Statement::RepeatLoop(_)
        | Statement::LocalAssignment(_)
        | Statement::GlobalDeclaration(_)
        | Statement::CompoundAssignment(_)
        // Prints nothing, so whatever it follows ends the output.
        | Statement::Error(_) => true,
        // Luau
        Statement::TypeDeclaration(declaration) => {
            matches!(declaration.value, TypeDeclarationValue::Alias(_))
        }
        Statement::DoBlock(_)
        | Statement::WhileLoop(_)
        | Statement::IfStatement(_)
        | Statement::NumericFor(_)
        | Statement::GenericFor(_)
        | Statement::FunctionDecl(_)
        | Statement::LocalFunction(_)
        | Statement::EmptyStatement(_)
        | Statement::Goto(_)
        | Statement::Label(_)
        | Statement::GlobalFunction(_)
        | Statement::GlobalStar(_)
        | Statement::Break(_) => false,
    }
}
