//! The traits lint rules implement, and the context their checks read.

use luck_ast::{Block, Expression, LastStatement, Statement};
use luck_core::LintConfig;
use luck_semantic::SemanticAnalysis;
use luck_token::Comment;

use crate::diagnostic::{Category, Finding, Severity};
use crate::node_type::NodeTypeSet;

/// The parsed document, its analysis, and the config, for one lint pass.
#[derive(Debug)]
pub(crate) struct LintContext<'document> {
    pub block: &'document Block,
    /// Every block in the document, `block` first, in the default
    /// visitor's pre-order.
    pub blocks: &'document [&'document Block],
    pub semantic: &'document SemanticAnalysis,
    pub source: &'document str,
    pub comments: &'document [Comment],
    pub config: &'document LintConfig,
}

/// A rule's identity and defaults. Its `WholeRule` or `NodeRule` impl does
/// the checking.
pub trait Rule: Send + Sync {
    /// Unique name, such as `unused_variable`, used in config and directives.
    fn name(&self) -> &'static str;

    fn category(&self) -> Category;

    fn default_severity(&self) -> Severity;

    fn description(&self) -> &'static str;
}

/// A rule that checks the whole document in one call, because it needs
/// state that spans nodes, such as scope stacks, statement sequences, or the
/// scope tree.
pub(crate) trait WholeRule: Rule {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding>;
}

/// A rule that gets one callback per node from the bus's shared walk
/// instead of walking the tree itself.
pub(crate) trait NodeRule: Rule {
    /// Node types this rule's hooks act on.
    ///
    /// Listing an extra type is harmless. Leaving one out silently skips
    /// those nodes, which the driver's debug check catches.
    fn node_types(&self) -> NodeTypeSet;

    fn on_statement(
        &self,
        _stmt: &Statement,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    fn on_expression(
        &self,
        _expr: &Expression,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    fn on_last_statement(
        &self,
        _last_stmt: &LastStatement,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }
}
