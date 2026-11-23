use std::sync::OnceLock;

use luck_ast::{
    Block, Call, Expression, GetSpan as _, Statement, SuffixedExpression, TableConstructor,
};
use luck_diagnostics::Severity;
use luck_parser::{Groupings, ParseResult};
use luck_semantic::{ResolvedCallee, SemanticAnalysis};
use luck_token::{Comment, LineIndex, Span};

use crate::config::LintConfig;
use crate::diagnostic::{Category, Finding};
use crate::directives::Directives;
use crate::driver::Analysis;
use crate::fix::FixKind;
use crate::local_flow::LocalFlow;
use crate::node_type::NodeTypeSet;
use crate::nodes::Nodes;

/// What every rule reads of one document. The lazily built parts are shared,
/// so the first rule to ask builds them for all.
#[derive(Debug)]
pub(crate) struct LintContext<'ast> {
    pub block: &'ast Block<'ast>,
    pub nodes: &'ast Nodes<'ast>,
    pub semantic: &'ast SemanticAnalysis<'ast>,
    pub directives: &'ast Directives,
    pub source: &'ast str,
    pub comments: &'ast [Comment],
    pub groupings: &'ast Groupings,
    /// Where each escape Lua 5.1 and Luau read as the character it escapes
    /// is, as the lexer found them.
    pub lenient_escapes: &'ast [Span],
    pub config: &'ast LintConfig,
    /// The kinds of fix the host applies, which are the only ones rules
    /// build.
    pub fixes: FixKind,
    pub local_flow: OnceLock<LocalFlow>,
    pub line_index: OnceLock<LineIndex>,
}

impl<'ast> LintContext<'ast> {
    pub(crate) fn new(
        parse: &'ast ParseResult<'_>,
        analysis: &'ast Analysis<'ast>,
        config: &'ast LintConfig,
        fixes: FixKind,
    ) -> Self {
        Self {
            block: &parse.chunk.block,
            nodes: analysis.nodes(),
            semantic: analysis.semantic(),
            directives: analysis.directives(),
            source: parse.source,
            comments: &parse.comments,
            groupings: &parse.groupings,
            lenient_escapes: &parse.lenient_escapes,
            config,
            fixes,
            local_flow: OnceLock::new(),
            line_index: OnceLock::new(),
        }
    }

    pub(crate) fn local_flow(&self) -> &LocalFlow {
        self.local_flow.get_or_init(|| LocalFlow::compute(self))
    }

    pub(crate) fn line_index(&self) -> &LineIndex {
        self.line_index.get_or_init(|| LineIndex::new(self.source, self.semantic.version()))
    }

    /// What `expr` spans with the parentheses it's written in, which the
    /// tree keeps only where they change what they hold.
    pub(crate) fn extent(&self, expr: &Expression<'_>) -> Span {
        let span = expr.span();
        self.groupings.around(span).unwrap_or(span)
    }

    /// The comments that sit inside `span`, in source order.
    pub(crate) fn comments_within(&self, span: Span) -> &'ast [Comment] {
        let comments = self.comments;
        let first = comments.partition_point(|comment| comment.span.start < span.start);
        let last = comments.partition_point(|comment| comment.span.end <= span.end);
        comments.get(first..last).unwrap_or_default()
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

    /// The kinds the rule's fixes come in, empty when it offers none. Some
    /// of its diagnostics may still have no fix.
    fn fix_kind(&self) -> FixKind;

    /// The rule's source on GitHub, at the release this linter is, where its
    /// doc comment and its `flags_` and `ignores_` tests document it.
    fn docs_url(&self) -> &'static str;
}

/// The last argument, when there is one, lists the kinds the rule's fixes
/// come in, as in `Fix | Suggestion`.
macro_rules! impl_rule {
    (
        $rule:ident,
        $name:literal,
        $category:ident,
        $severity:ident,
        $description:literal
        $(, $($fix_kind:ident)|+)?
    ) => {
        impl $crate::rule::Rule for $rule {
            fn name(&self) -> &'static str {
                $name
            }
            fn category(&self) -> $crate::diagnostic::Category {
                $crate::diagnostic::Category::$category
            }
            fn default_severity(&self) -> luck_diagnostics::Severity {
                luck_diagnostics::Severity::$severity
            }
            fn description(&self) -> &'static str {
                $description
            }
            fn fix_kind(&self) -> $crate::fix::FixKind {
                $crate::fix::FixKind::empty() $($(.union($crate::fix::FixKind::$fix_kind))+)?
            }
            fn docs_url(&self) -> &'static str {
                concat!(
                    env!("CARGO_PKG_REPOSITORY"),
                    "/blob/v",
                    env!("CARGO_PKG_VERSION"),
                    "/crates/luck_linter/src/rules/",
                    $name,
                    ".rs",
                )
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

/// An assignment target's chain reads everything but what its last suffix
/// stores to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainPosition {
    Value,
    AssignmentTarget,
}

/// A fix's replacement must parse as a statement in statement position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallPosition {
    Statement,
    Expression,
}

/// A call the bus dispatches, where it stands, and what it calls.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallSite<'ast> {
    pub call: Call<'ast>,
    pub position: CallPosition,
    /// As [`SemanticAnalysis::resolve_callee`] gives it. The bus resolves a
    /// chain's calls in one pass, once for every rule.
    pub callee: Option<ResolvedCallee<'ast>>,
}

/// A rule that gets one callback per node from the bus's shared walk
/// instead of walking the tree itself.
pub(crate) trait NodeRule: Rule {
    /// Listing an extra type is harmless. Leaving one out silently skips
    /// those nodes, which the driver's debug check catches.
    fn node_types(&self) -> NodeTypeSet;

    /// Never gets a `Statement::FunctionCall`, which goes to [`Self::on_call`].
    fn on_stmt(
        &self,
        _stmt: &Statement<'_>,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Never gets an `Expression::Suffixed`, which goes to
    /// [`Self::on_chain`], or an `Expression::TableConstructor`, which goes
    /// to [`Self::on_table`].
    fn on_expr(
        &self,
        _expr: &Expression<'_>,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Runs for every table constructor, as an expression or a `f{...}`
    /// argument.
    fn on_table(
        &self,
        _table: &TableConstructor<'_>,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Runs for every chain, in an expression, a call statement, or an
    /// assignment target.
    fn on_chain(
        &self,
        _chain: &SuffixedExpression<'_>,
        _position: ChainPosition,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }

    /// Runs for every call in every chain, the one a call statement makes
    /// included. A rule that subscribes to only one of `FunctionCallStmt`
    /// and `FunctionCallExpr` must check the site's `position`, because the
    /// driver's debug check calls this for both.
    fn on_call(
        &self,
        _site: &CallSite<'_>,
        _context: &LintContext<'_>,
        _findings: &mut Vec<Finding>,
    ) {
    }
}
