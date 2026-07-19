use luck_ast::Expression;
use luck_ast::expr::Var;
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;

use crate::diagnostic::*;
use crate::rule::{LintContext, Rule};

pub struct AlmostSwapped;

impl Rule for AlmostSwapped {
    fn name(&self) -> &'static str {
        "almost_swapped"
    }
    fn category(&self) -> Category {
        Category::Correctness
    }
    fn default_severity(&self) -> Severity {
        Severity::Error
    }
    fn description(&self) -> &'static str {
        "looks like a failed variable swap (use a, b = b, a)"
    }

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic> {
        let block = ctx.block;
        let _semantic = ctx.semantic;
        let source = ctx.source;
        let _comments = ctx.comments;
        let mut checker = SwapChecker {
            source,
            diagnostics: Vec::new(),
        };
        checker.check_block(block);
        checker.diagnostics
    }
}

struct SwapChecker<'a> {
    source: &'a str,
    diagnostics: Vec<LintDiagnostic>,
}

fn var_name<'a>(var: &Var, source: &'a str) -> Option<&'a str> {
    if let Var::Name(token) = var {
        Some(&source[token.span.start as usize..token.span.end as usize])
    } else {
        None
    }
}

fn expr_var_name<'a>(expr: &Expression, source: &'a str) -> Option<&'a str> {
    if let Expression::Var(var) = expr {
        var_name(var, source)
    } else {
        None
    }
}

impl SwapChecker<'_> {
    fn check_block(&mut self, block: &Block) {
        let stmts = &block.stmts;
        for i in 0..stmts.len().saturating_sub(1) {
            if let (luck_ast::Statement::Assignment(a1), luck_ast::Statement::Assignment(a2)) =
                (&stmts[i], &stmts[i + 1])
            {
                if a1.targets.len() == 1
                    && a2.targets.len() == 1
                    && a1.values.len() == 1
                    && a2.values.len() == 1
                    && let (Some(t1), Some(v1), Some(t2), Some(v2)) = (
                        a1.targets.first().and_then(|v| var_name(v, self.source)),
                        a1.values
                            .first()
                            .and_then(|e| expr_var_name(e, self.source)),
                        a2.targets.first().and_then(|v| var_name(v, self.source)),
                        a2.values
                            .first()
                            .and_then(|e| expr_var_name(e, self.source)),
                    )
                {
                    // This is the `a = b; b = a` pattern, where t1=a, v1=b, t2=b, v2=a.
                    if t1 == v2 && t2 == v1 {
                        self.diagnostics.push(LintDiagnostic::new("almost_swapped", format!(
                                "'{t1} = {v1}; {t2} = {v2}' does not swap; use '{t1}, {t2} = {t2}, {t1}'"
                            ), a1.span.merge(a2.span)).with_help(format!("use '{t1}, {t2} = {t2}, {t1}' for simultaneous swap")));
                    }
                }
            }

            // Recurse into nested blocks via the default visitor walk.
            self.visit_statement(&stmts[i]);
        }
        if let Some(last) = stmts.last() {
            self.visit_statement(last);
        }
    }
}

impl<'ast> Visitor<'ast> for SwapChecker<'_> {
    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        match stmt {
            luck_ast::Statement::DoBlock(d) => self.check_block(&d.block),
            luck_ast::Statement::WhileLoop(w) => self.check_block(&w.block),
            luck_ast::Statement::RepeatLoop(r) => self.check_block(&r.block),
            luck_ast::Statement::NumericFor(n) => self.check_block(&n.block),
            luck_ast::Statement::GenericFor(g) => self.check_block(&g.block),
            luck_ast::Statement::IfStatement(i) => {
                self.check_block(&i.block);
                for clause in &i.elseif_clauses {
                    self.check_block(&clause.block);
                }
                if let Some(else_clause) = &i.else_clause {
                    self.check_block(&else_clause.block);
                }
            }
            luck_ast::Statement::FunctionDecl(f) => self.check_block(&f.body.block),
            luck_ast::Statement::LocalFunction(f) => self.check_block(&f.body.block),
            _ => {}
        }
    }
}
