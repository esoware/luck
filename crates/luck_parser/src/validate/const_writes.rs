//! Writes to read-only bindings: `<const>`/`<close>` locals (5.4+), `<const>`
//! globals (5.5), Luau `const` bindings, and 5.5 for-loop control variables.

use luck_ast::{AttributedName, Block, FunctionBody, Punctuated, Statement, Type, Var, Visitor};
use luck_token::{LuaVersion, Token};

use crate::ParseError;

pub(super) fn check(block: &Block, version: LuaVersion) -> Vec<ParseError> {
    let mut checker = ConstWriteChecker { version, bindings: Vec::new(), errors: Vec::new() };
    checker.visit_block(block);
    checker.errors
}

/// Function boundaries keep the scope stack, since Lua also rejects writes to
/// a const upvalue.
struct ConstWriteChecker<'ast> {
    version: LuaVersion,
    /// Visible names, innermost last, each with whether it's read-only.
    bindings: Vec<(&'ast str, bool)>,
    errors: Vec<ParseError>,
}

impl<'ast> ConstWriteChecker<'ast> {
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

    /// Whether the innermost visible binding of `name` is read-only. A name
    /// with no binding is a global, which is writable.
    fn is_readonly(&self, name: &str) -> bool {
        self.bindings
            .iter()
            .rev()
            .find(|(declared, _)| *declared == name)
            .is_some_and(|&(_, is_readonly)| is_readonly)
    }

    fn check_write(&mut self, token: &Token) {
        if let Some(name) = token.identifier()
            && self.is_readonly(name)
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

impl<'ast> Visitor<'ast> for ConstWriteChecker<'ast> {
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
