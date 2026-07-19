use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::visitor::Visitor;
use luck_token::TokenKind;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

/// `_` is a regular identifier in Lua, but convention is "value I'm
/// throwing away". Reading from it usually means the author forgot
/// which binding they meant, or that an outer `_` is shadowed by a
/// nearer one. Walk expressions, skipping declaration/target/parameter
/// positions which are always non-reads.
pub struct PlaceholderRead;

impl Rule for PlaceholderRead {
    fn name(&self) -> &'static str {
        "placeholder_read"
    }
    fn category(&self) -> Category {
        Category::Suspicious
    }
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }
    fn description(&self) -> &'static str {
        "use of `_` as a value; convention reserves it for ignored bindings"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = PlaceholderChecker {
            diagnostics: Vec::new(),
        };
        checker.visit_block(block);
        let _ = source;
        checker.diagnostics
    }
}

struct PlaceholderChecker {
    diagnostics: Vec<LintDiagnostic>,
}

fn is_placeholder_name(kind: &TokenKind) -> bool {
    if let TokenKind::Identifier(name) = kind {
        return name == "_";
    }
    false
}

impl PlaceholderChecker {
    /// Visit `Var` in a non-target position (i.e. as a value). Walks
    /// into compound `Var::Index` / `Var::FieldAccess` to flag inner
    /// reads like `_.field` while also recursing into index keys.
    fn visit_var_as_read(&mut self, var: &Var) {
        match var {
            Var::Name(token) => {
                if is_placeholder_name(&token.kind) {
                    self.diagnostics.push(
                        LintDiagnostic::new(
                            "placeholder_read",
                            "read of `_`; convention is to use `_` only for ignored bindings"
                                .to_string(),
                            token.span,
                        )
                        .with_help(
                            "rename the binding or pull the value into a meaningful local"
                                .to_string(),
                        ),
                    );
                }
            }
            Var::Index(idx) => {
                self.visit_expression(&idx.prefix);
                self.visit_expression(&idx.index);
            }
            Var::FieldAccess(fld) => {
                self.visit_expression(&fld.prefix);
            }
        }
    }
}

impl<'ast> Visitor<'ast> for PlaceholderChecker {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::Var(var) = expr {
            self.visit_var_as_read(var);
            return;
        }
        self.walk_expression(expr);
    }

    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        match stmt {
            // Assignment LHS: skip the targets (they're writes), visit
            // values (they may contain reads).
            luck_ast::Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    // Still walk into Index/FieldAccess prefixes - those
                    // are reads even on the LHS of an assignment.
                    if let Var::Index(idx) = var {
                        self.visit_expression(&idx.prefix);
                        self.visit_expression(&idx.index);
                    } else if let Var::FieldAccess(fld) = var {
                        self.visit_expression(&fld.prefix);
                    }
                }
                for expr in assignment.values.iter() {
                    self.visit_expression(expr);
                }
            }
            // Local binding names are declarations, not reads. Visit
            // initializer expressions only.
            luck_ast::Statement::LocalAssignment(local) => {
                if let Some((_, exprs)) = &local.equal_and_exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
            }
            // Compound assignment (`x += y`): the var is read-then-write,
            // but we still skip it for placeholder semantics - `_ += 1`
            // is rare and clearly self-referential.
            luck_ast::Statement::CompoundAssignment(compound) => {
                if let Var::Index(idx) = &compound.var {
                    self.visit_expression(&idx.prefix);
                    self.visit_expression(&idx.index);
                } else if let Var::FieldAccess(fld) = &compound.var {
                    self.visit_expression(&fld.prefix);
                }
                self.visit_expression(&compound.expr);
            }
            // Generic-for binding names are declarations; visit the
            // iterator expressions and body only.
            luck_ast::Statement::GenericFor(generic_for) => {
                for expr in generic_for.exprs.iter() {
                    self.visit_expression(expr);
                }
                self.visit_block(&generic_for.block);
            }
            // Numeric-for binding name is also a declaration.
            luck_ast::Statement::NumericFor(num_for) => {
                self.visit_expression(&num_for.start);
                self.visit_expression(&num_for.limit);
                if let Some((_, step)) = &num_for.comma2_and_step {
                    self.visit_expression(step);
                }
                self.visit_block(&num_for.block);
            }
            // Function parameters in any declaration form are bindings,
            // not reads; the body is recursed by the default walker.
            luck_ast::Statement::LocalFunction(_)
            | luck_ast::Statement::FunctionDecl(_)
            | luck_ast::Statement::GlobalFunction(_) => {
                self.walk_statement(stmt);
            }
            // All other shapes have no special "skip these tokens"
            // logic - fall back to the default traversal.
            luck_ast::Statement::FunctionCall(_)
            | luck_ast::Statement::DoBlock(_)
            | luck_ast::Statement::WhileLoop(_)
            | luck_ast::Statement::RepeatLoop(_)
            | luck_ast::Statement::IfStatement(_)
            | luck_ast::Statement::EmptyStatement(_)
            | luck_ast::Statement::Goto(_)
            | luck_ast::Statement::Label(_)
            | luck_ast::Statement::GlobalDeclaration(_)
            | luck_ast::Statement::GlobalStar(_)
            | luck_ast::Statement::Break(_)
            | luck_ast::Statement::TypeDeclaration(_)
            | luck_ast::Statement::Error(_) => {
                self.walk_statement(stmt);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luck_token::LuaVersion;

    fn run(source: &str) -> Vec<LintDiagnostic> {
        crate::test_support::run_rule(&PlaceholderRead, source, LuaVersion::Lua54)
    }

    #[test]
    fn flags_print_of_underscore() {
        let diags = run("print(_)");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn ignores_local_declaration() {
        let diags = run("local _ = f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_assignment_target() {
        let diags = run("_ = f()");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_read_inside_generic_for_body() {
        let diags = run("for _, v in pairs(t) do print(_) end");
        assert_eq!(
            diags.len(),
            1,
            "binding should not fire, body read should: {diags:?}"
        );
    }

    #[test]
    fn ignores_numeric_for_binding() {
        let diags = run("for _ = 1, 10 do end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn ignores_function_parameter() {
        let diags = run("local function f(_) end");
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn flags_read_inside_function_body() {
        let diags = run("local function f(_) return _ end");
        assert_eq!(diags.len(), 1, "{diags:?}");
    }
}
