use std::sync::OnceLock;

use luck_ast::{Block, Expression, FunctionCall, Statement, TableConstructor};
use luck_core::LintConfig;
use luck_semantic::SemanticAnalysis;
use luck_token::Comment;

use crate::diagnostic::{Category, Finding, Severity};
use crate::local_flow::LocalFlow;
use crate::node_type::NodeTypeSet;
use crate::uninitialized::UninitializedLocals;

#[derive(Debug)]
pub(crate) struct LintContext<'document> {
    pub block: &'document Block,
    /// Every block in the document, `block` first, in the default
    /// visitor's pre-order.
    pub blocks: &'document [&'document Block],
    /// `block`, then the body of every function in it.
    pub function_blocks: &'document [&'document Block],
    pub uninitialized_locals: &'document UninitializedLocals,
    pub semantic: &'document SemanticAnalysis,
    pub source: &'document str,
    pub comments: &'document [Comment],
    pub config: &'document LintConfig,
    pub local_flow: OnceLock<LocalFlow>,
}

impl LintContext<'_> {
    pub(crate) fn local_flow(&self) -> &LocalFlow {
        self.local_flow.get_or_init(|| LocalFlow::compute(self))
    }
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

macro_rules! impl_rule {
    ($rule:ident, $name:literal, $category:ident, $severity:ident, $description:literal) => {
        impl $crate::rule::Rule for $rule {
            fn name(&self) -> &'static str {
                $name
            }
            fn category(&self) -> $crate::diagnostic::Category {
                $crate::diagnostic::Category::$category
            }
            fn default_severity(&self) -> $crate::diagnostic::Severity {
                $crate::diagnostic::Severity::$severity
            }
            fn description(&self) -> &'static str {
                $description
            }
        }
    };
}

pub(crate) use impl_rule;

/// A rule that checks the whole document in one call, because it needs
/// state that spans nodes, such as scope stacks, statement sequences, or the
/// scope tree.
pub(crate) trait WholeRule: Rule {
    fn check(&self, context: &LintContext<'_>) -> Vec<Finding>;
}

/// A fix's replacement must parse as a statement in statement position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallPosition {
    Statement,
    Expression,
}

/// A rule that gets one callback per node from the bus's shared walk
/// instead of walking the tree itself.
pub(crate) trait NodeRule: Rule {
    /// Listing an extra type is harmless. Leaving one out silently skips
    /// those nodes, which the driver's debug check catches.
    fn node_types(&self) -> NodeTypeSet;

    /// Never gets a `Statement::FunctionCall`, which goes to [`Self::on_call`].
    fn on_stmt(&self, _stmt: &Statement, _context: &LintContext<'_>, _findings: &mut Vec<Finding>) {
    }

    /// Never gets an `Expression::FunctionCall`, which goes to
    /// [`Self::on_call`], or an `Expression::TableConstructor`, which goes
    /// to [`Self::on_table`].
    fn on_expr(
        &self,
        _expr: &Expression,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Runs for every table constructor, as an expression or a `f{...}`
    /// argument.
    fn on_table(
        &self,
        _table: &TableConstructor,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Runs for every call the bus reaches, in either position. A rule that
    /// subscribes to only one of `FunctionCallStmt` and `FunctionCallExpr`
    /// must check `position`, because the driver's debug check calls this
    /// for both.
    fn on_call(
        &self,
        _call: &FunctionCall,
        _position: CallPosition,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }
}
