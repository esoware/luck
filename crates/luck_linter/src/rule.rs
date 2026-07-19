use luck_ast::shared::Block;
use luck_semantic::SemanticAnalysis;
use luck_token::Comment;

use crate::LintConfig;
use crate::diagnostic::{Category, LintDiagnostic, Severity};

/// Everything a rule can see for one lint pass over one document.
pub struct LintContext<'a> {
    pub block: &'a Block,
    pub semantic: &'a SemanticAnalysis,
    pub source: &'a str,
    pub comments: &'a [Comment],
    pub config: &'a LintConfig,
}

/// A lint rule that checks code for issues.
pub trait Rule: Send + Sync {
    /// Unique name of this rule (e.g. "unused_variable").
    fn name(&self) -> &'static str;

    fn category(&self) -> Category;

    fn default_severity(&self) -> Severity;

    fn description(&self) -> &'static str;

    fn check(&self, ctx: &LintContext) -> Vec<LintDiagnostic>;
}

/// A rule driven by the shared single-pass walk (`bus::run`): instead of
/// walking the AST itself, it receives one callback per node. Rules whose
/// logic is node-local implement this and delegate `Rule::check` to
/// [`crate::bus::run_single`]; rules that need traversal state (scope
/// stacks, statement sequences, CFG) stay whole-tree `Rule`s.
pub trait NodeRule: Rule {
    fn on_statement(
        &self,
        _stmt: &luck_ast::stmt::Statement,
        _ctx: &LintContext,
        _out: &mut Vec<LintDiagnostic>,
    ) {
    }

    fn on_expression(
        &self,
        _expr: &luck_ast::expr::Expression,
        _ctx: &LintContext,
        _out: &mut Vec<LintDiagnostic>,
    ) {
    }
}
