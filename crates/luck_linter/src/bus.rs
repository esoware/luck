//! Single-pass dispatch for node-local rules.
//!
//! One pre-order walk over the AST calls every subscribed
//! [`NodeRule`]'s hooks, replacing N per-rule traversals with one.
//! Each rule collects into its own slot so per-rule diagnostic order is
//! identical to the rule walking the tree by itself.

use luck_ast::expr::Expression;
use luck_ast::stmt::Statement;
use luck_ast::visitor::Visitor;

use crate::diagnostic::LintDiagnostic;
use crate::rule::{LintContext, NodeRule};

/// Run `rules` in one shared walk; returns one diagnostics vec per rule,
/// in the same order as `rules`.
pub(crate) fn run(rules: &[&dyn NodeRule], ctx: &LintContext) -> Vec<Vec<LintDiagnostic>> {
    let mut bus = NodeBus {
        rules,
        ctx,
        out: vec![Vec::new(); rules.len()],
    };
    bus.visit_block(ctx.block);
    bus.out
}

/// Run one node rule by itself: the `Rule::check` implementation for
/// every converted rule (used directly by per-rule tests).
pub fn run_single(rule: &dyn NodeRule, ctx: &LintContext) -> Vec<LintDiagnostic> {
    run(&[rule], ctx)
        .pop()
        .expect("run returns one slot per rule")
}

struct NodeBus<'a, 'ctx> {
    rules: &'a [&'a dyn NodeRule],
    ctx: &'a LintContext<'ctx>,
    out: Vec<Vec<LintDiagnostic>>,
}

impl Visitor for NodeBus<'_, '_> {
    fn visit_statement(&mut self, stmt: &Statement) {
        for (slot, rule) in self.rules.iter().enumerate() {
            rule.on_statement(stmt, self.ctx, &mut self.out[slot]);
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &Expression) {
        for (slot, rule) in self.rules.iter().enumerate() {
            rule.on_expression(expr, self.ctx, &mut self.out[slot]);
        }
        self.walk_expression(expr);
    }
}
