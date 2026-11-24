use std::cell::{OnceCell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::{iter, ptr};

use luck_ast::{AstKind, Block, Chunk, Expression, GetSpan as _, Name, Var};
use luck_parser::{Groupings, ParseResult};
use luck_semantic::{AstNode, AstNodes, NodeId, ScopeId, Scoping};
use luck_stdlib::{ResolvedCallee, SemanticAnalysis};
use luck_token::{Comment, LineIndex, LuaVersion, Span};
use luck_value::ValueContext;

use crate::cfg::FlowGraph;
use crate::diagnostic::Finding;
use crate::directives::Directives;
use crate::fix::FixKind;
use crate::linter::Analysis;
use crate::local_flow::LocalFlow;

/// What every rule reads of one file, shared by all, as oxlint's
/// `ContextHost`. The parts built on first use are built once, for every
/// rule that asks.
#[derive(Debug)]
pub(crate) struct ContextHost<'ast> {
    pub chunk: &'ast Chunk<'ast>,
    pub semantic: &'ast SemanticAnalysis<'ast>,
    pub nodes: &'ast AstNodes<'ast>,
    pub directives: &'ast Directives,
    pub source: &'ast [u8],
    pub comments: &'ast [Comment],
    pub groupings: &'ast Groupings,
    /// Where each escape Lua 5.1 and Luau read as the character it escapes
    /// is, as the lexer found them.
    pub lenient_escapes: &'ast [Span],
    /// The kinds of fix the host applies, which are the only ones rules
    /// build.
    pub fixes: FixKind,
    /// Each function's control-flow graph, by its scope's index, which
    /// [`Self::cfg`] builds.
    pub graphs: OnceCell<Box<[OnceCell<Box<FlowGraph>>]>>,
    pub local_flow: OnceCell<LocalFlow>,
    pub line_index: OnceCell<LineIndex>,
    /// The chain a rule last asked [`Self::callee`] about. Every rule that
    /// runs on a node runs before the pass moves on, so they share one
    /// resolution of its calls.
    pub resolved_chain: RefCell<ResolvedChain<'ast>>,
}

#[derive(Debug, Default)]
pub(crate) struct ResolvedChain<'ast> {
    chain: Option<NodeId>,
    /// What each suffix calls, by its index: `None` for one that's no call,
    /// or calls nothing the stdlib knows.
    callees: Vec<Option<ResolvedCallee<'ast>>>,
}

impl<'ast> ContextHost<'ast> {
    pub(crate) fn new(
        parse: &'ast ParseResult<'_>,
        analysis: &'ast Analysis<'ast>,
        fixes: FixKind,
    ) -> Self {
        let semantic = analysis.semantic();
        Self {
            chunk: &parse.chunk,
            semantic,
            nodes: semantic.semantic().nodes(),
            directives: analysis.directives(),
            source: parse.source,
            comments: &parse.comments,
            groupings: &parse.groupings,
            lenient_escapes: &parse.lenient_escapes,
            fixes,
            graphs: OnceCell::new(),
            local_flow: OnceCell::new(),
            line_index: OnceCell::new(),
            resolved_chain: RefCell::default(),
        }
    }

    pub(crate) const fn scoping(&self) -> &'ast Scoping {
        self.semantic.scoping()
    }

    /// The function scope the code in `scope` runs in, the chunk's for code
    /// outside every function.
    pub(crate) fn function_of(&self, scope: ScopeId) -> ScopeId {
        self.scoping().scope(scope).function
    }

    /// The control-flow graph of the function the code in `scope` runs in,
    /// which every rule that asks shares.
    pub(crate) fn cfg(&self, scope: ScopeId) -> Option<&FlowGraph> {
        let function = self.function_of(scope);
        let graphs = self.graphs.get_or_init(|| {
            iter::repeat_with(OnceCell::new).take(self.scoping().scopes().len()).collect()
        });
        let block = self.function_block(function)?;
        Some(graphs[function.index()].get_or_init(|| Box::new(FlowGraph::of_block(block, self))))
    }

    /// The block of the function or chunk that opens `function`.
    pub(crate) fn function_block(&self, function: ScopeId) -> Option<&'ast Block<'ast>> {
        let opener = self.nodes.kind(self.scoping().scope(function).node);
        if let AstKind::FunctionBody(body) = opener {
            return Some(&body.block);
        }
        opener.as_chunk().map(|chunk| &chunk.block)
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

    /// What the call at `index` in `node`, a chain, calls, as
    /// [`SemanticAnalysis::chain_callees`] resolves it, once for every rule
    /// that asks. `None` for a suffix that's no call.
    pub(crate) fn callee(
        &self,
        node: &AstNode<'ast>,
        index: usize,
    ) -> Option<ResolvedCallee<'ast>> {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return None;
        };
        let mut resolved = self.resolved_chain.borrow_mut();
        if resolved.chain != Some(node.id()) {
            let mut callees = self.semantic.chain_callees(chain);
            resolved.chain = Some(node.id());
            resolved.callees.clear();
            resolved.callees.extend((0..chain.suffixes.len()).map(|i| callees.callee(i)));
        }
        resolved.callees[index]
    }

    /// Whether `node` is a statement of its block, as a chain that a call
    /// statement makes is.
    pub(crate) fn is_statement(&self, node: &AstNode<'_>) -> bool {
        matches!(self.nodes.parent_kind(node.id()), Some(AstKind::Block(_)))
    }

    /// Whether `node`, a chain, is what an assignment stores to, whose last
    /// suffix it writes rather than reads.
    pub(crate) fn is_assignment_target(&self, node: &AstNode<'_>) -> bool {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return false;
        };
        let is_chain = |var: &Var<'_>| matches!(var, Var::Suffixed(target) if ptr::eq(&raw const **target, chain));
        let parent = self.nodes.parent_kind(node.id());
        if let Some(AstKind::Assignment(assignment)) = parent {
            return assignment.targets.iter().any(is_chain);
        }
        // Luau
        matches!(parent, Some(AstKind::CompoundAssignment(compound)) if is_chain(&compound.var))
    }
}

/// A free name reads a global, or a field of a local `_ENV`, so either may
/// run `__index`.
impl ValueContext for ContextHost<'_> {
    fn version(&self) -> LuaVersion {
        self.semantic.version()
    }

    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        !self.scoping().resolves_to_local(name)
    }
}

/// One rule's view of a file: the [`ContextHost`] every rule shares, and
/// the findings this rule reports, as oxlint's `LintContext`.
#[derive(Debug)]
pub(crate) struct LintContext<'ast> {
    host: Rc<ContextHost<'ast>>,
    findings: RefCell<Vec<Finding>>,
}

impl<'ast> LintContext<'ast> {
    pub(crate) const fn new(host: Rc<ContextHost<'ast>>) -> Self {
        Self { host, findings: RefCell::new(Vec::new()) }
    }

    pub(crate) fn diagnostic(&self, finding: Finding) {
        self.findings.borrow_mut().push(finding);
    }

    pub(crate) fn into_findings(self) -> Vec<Finding> {
        self.findings.into_inner()
    }
}

impl<'ast> Deref for LintContext<'ast> {
    type Target = ContextHost<'ast>;

    fn deref(&self) -> &Self::Target {
        &self.host
    }
}

impl ValueContext for LintContext<'_> {
    fn version(&self) -> LuaVersion {
        self.host.semantic.version()
    }

    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        self.host.is_global_reference(name)
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaTarget;

    use super::*;
    use crate::fixtures::with_host;

    #[test]
    fn every_scope_of_a_function_shares_one_graph() {
        let source = "\
do local x = 1 end
local function f(a)
  while a do a = a - 1 end
  for i = 1, 2 do print(i) end
end";
        with_host(source, LuaTarget::Lua54, |host| {
            let graphs: Vec<(ScopeId, *const FlowGraph)> = host
                .nodes
                .iter()
                .map(AstNode::scope)
                .map(|scope| (host.function_of(scope), ptr::from_ref(host.cfg(scope).unwrap())))
                .collect();
            for &(function, graph) in &graphs {
                for &(other_function, other_graph) in &graphs {
                    assert_eq!(function == other_function, ptr::eq(graph, other_graph));
                }
            }
            let mut functions: Vec<ScopeId> =
                graphs.iter().map(|&(function, _)| function).collect();
            functions.dedup();
            assert!(functions.len() > 1, "{functions:?}");
        });
    }

    #[test]
    fn a_callee_resolves_on_the_chain_asked_about() {
        let source = "string.format('%d', 1)
os.time()";
        with_host(source, LuaTarget::Lua54, |host| {
            let chains: Vec<&AstNode<'_>> = host
                .nodes
                .iter()
                .filter(|node| matches!(node.kind(), AstKind::SuffixedExpression(_)))
                .collect();
            let names: Vec<Option<String>> = [0, 1, 0]
                .into_iter()
                .map(|i| host.callee(chains[i], 1).map(|callee| callee.name()))
                .collect();
            let expected =
                ["string.format", "os.time", "string.format"].map(|name| Some(name.into()));
            assert_eq!(names, expected);
            assert!(host.callee(chains[1], 0).is_none(), "a field is no call");
        });
    }

    #[test]
    fn a_free_name_reads_a_global_and_a_local_doesnt() {
        let source = "\
local x = 1
print(x, y)";
        with_host(source, LuaTarget::Lua54, |host| {
            let reads: Vec<(&str, bool)> = host
                .nodes
                .iter()
                .filter_map(|node| node.kind().as_name())
                .filter(|name| host.scoping().reference_of(name).is_some())
                .map(|name| (name.text.as_str(), host.is_global_reference(name)))
                .collect();
            assert_eq!(reads, [("print", true), ("x", false), ("y", true)]);
        });
    }
}
