use std::cell::{OnceCell, RefCell};
use std::ops::Deref;
use std::{iter, ptr};

use luck_ast::{
    AstKind, Block, Call, Expression, GetSpan as _, LocalAssignment, Name, Suffix,
    SuffixedExpression, Var,
};
use luck_parser::Groupings;
use luck_semantic::{AstNode, AstNodes, NodeId, Reference, ScopeId, Scoping, Symbol};
use luck_stdlib::{ResolvedCallee, SemanticAnalysis};
use luck_token::{Comment, LineIndex, Span};
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
    pub chunk: &'ast Block<'ast>,
    pub semantic: &'ast SemanticAnalysis<'ast>,
    pub nodes: &'ast AstNodes<'ast>,
    pub directives: &'ast Directives,
    pub source: &'ast [u8],
    pub comments: &'ast [Comment],
    pub groupings: &'ast Groupings,
    /// Where each unknown escape is, such as `\q`, which reads as the
    /// character it escapes, as the lexer found them.
    pub lenient_escapes: &'ast [Span],
    /// The kinds of fix the host applies, which are the only ones rules
    /// build.
    pub fixes: FixKind,
    /// Each function's control-flow graph, by its scope's index, which
    /// [`Self::cfg`] builds.
    graphs: OnceCell<Box<[OnceCell<Box<FlowGraph>>]>>,
    local_flow: OnceCell<LocalFlow>,
    line_index: OnceCell<LineIndex>,
    /// The chain a rule last asked [`Self::callee`] about. Every rule that
    /// runs on a node runs before the pass moves on, so they share one
    /// resolution of its calls.
    resolved_chain: RefCell<ResolvedChain<'ast>>,
}

#[derive(Debug, Default)]
struct ResolvedChain<'ast> {
    chain: Option<NodeId>,
    /// What each suffix calls, by its index: `None` for one that's no call,
    /// or calls nothing the stdlib knows.
    callees: Vec<Option<ResolvedCallee<'ast>>>,
}

impl<'ast> ContextHost<'ast> {
    pub(crate) fn new(analysis: &'ast Analysis<'ast>, fixes: FixKind) -> Self {
        let parse = analysis.parse();
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
    fn function_block(&self, function: ScopeId) -> Option<&'ast Block<'ast>> {
        let opener = self.nodes.kind(self.scoping().scope(function).node);
        if let AstKind::FunctionBody(body) = opener { Some(&body.block) } else { opener.as_block() }
    }

    pub(crate) fn local_flow(&self) -> &LocalFlow {
        self.local_flow.get_or_init(|| LocalFlow::compute(self))
    }

    pub(crate) fn line_index(&self) -> &LineIndex {
        self.line_index.get_or_init(|| LineIndex::new(self.source))
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

    /// When `node` is a call statement's chain, the index of its last
    /// suffix, whose call drops what it returns.
    pub(crate) fn statement_call(&self, node: &AstNode<'_>) -> Option<usize> {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return None;
        };
        matches!(self.nodes.parent_kind(node.id()), Some(AstKind::Block(_)))
            .then(|| chain.suffixes.len() - 1)
    }

    /// The calls of `node`, a chain, whose results the code uses: every one
    /// but a call statement's last.
    pub(crate) fn used_calls<'node>(
        &self,
        node: &AstNode<'node>,
    ) -> impl Iterator<Item = Call<'node>> {
        let dropped = self.statement_call(node);
        node.kind()
            .as_suffixed_expr()
            .into_iter()
            .flat_map(SuffixedExpression::calls)
            .filter(move |&(index, _)| Some(index) != dropped)
            .map(|(_, call)| call)
    }

    /// The suffixes `node`, a chain, reads through: all of them, but for the
    /// last of an assignment's target, which it stores to.
    pub(crate) fn read_suffixes<'node>(&self, node: &AstNode<'node>) -> &'node [Suffix<'node>] {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return &[];
        };
        let suffixes: &[Suffix<'_>] = &chain.suffixes;
        if self.is_assignment_target(node) { &suffixes[..suffixes.len() - 1] } else { suffixes }
    }

    /// The `local` assignment that declares `symbol`, and where among its
    /// names the symbol is, or `None` for a symbol something else declares.
    pub(crate) fn local_declaration(
        &self,
        symbol: &Symbol,
    ) -> Option<(&'ast LocalAssignment<'ast>, usize)> {
        let (AstKind::TypedName(local_name), Some(AstKind::LocalAssignment(local))) =
            (self.nodes.kind(symbol.declaration), self.nodes.parent_kind(symbol.declaration))
        else {
            return None;
        };
        let position = local.names.iter().position(|name| ptr::eq(name, local_name))?;
        Some((local, position))
    }

    /// Whether `node`, a chain, is what an assignment stores to, whose last
    /// suffix it writes rather than reads.
    fn is_assignment_target(&self, node: &AstNode<'_>) -> bool {
        let AstKind::SuffixedExpression(chain) = node.kind() else {
            return false;
        };
        let is_chain = |var: &Var<'_>| matches!(var, Var::Suffixed(target) if ptr::eq(&raw const **target, chain));
        let parent = self.nodes.parent_kind(node.id());
        if let Some(AstKind::Assignment(assignment)) = parent {
            return assignment.targets.iter().any(is_chain);
        }
        matches!(parent, Some(AstKind::CompoundAssignment(compound)) if is_chain(&compound.var))
    }

    /// Whether `reference` is the name a field or index write goes through
    /// directly, as the `x` of `x.foo = 1`, which the scope tree records as a
    /// plain read.
    pub(crate) fn is_field_write_prefix(&self, reference: &Reference) -> bool {
        let Some(chain_node) = self.nodes.parent_node(reference.node) else {
            return false;
        };
        let AstKind::SuffixedExpression(chain) = chain_node.kind() else {
            return false;
        };
        chain.suffixes.len() == 1
            && matches!(&chain.prefix, Expression::Name(name) if name.span == reference.span)
            && self.is_assignment_target(chain_node)
    }
}

/// A free name reads a global, which may run `__index`.
impl ValueContext for ContextHost<'_> {
    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        !self.scoping().resolves_to_local(name)
    }
}

/// One rule's view of a file: the [`ContextHost`] every rule shares, and
/// the findings this rule reports, as oxlint's `LintContext`.
#[derive(Debug)]
pub(crate) struct LintContext<'host, 'ast> {
    host: &'host ContextHost<'ast>,
    findings: RefCell<Vec<Finding>>,
}

impl<'host, 'ast> LintContext<'host, 'ast> {
    pub(crate) const fn new(host: &'host ContextHost<'ast>) -> Self {
        Self { host, findings: RefCell::new(Vec::new()) }
    }

    pub(crate) fn diagnostic(&self, finding: Finding) {
        self.findings.borrow_mut().push(finding);
    }

    pub(crate) fn into_findings(self) -> Vec<Finding> {
        self.findings.into_inner()
    }
}

impl<'ast> Deref for LintContext<'_, 'ast> {
    type Target = ContextHost<'ast>;

    fn deref(&self) -> &Self::Target {
        self.host
    }
}

impl ValueContext for LintContext<'_, '_> {
    fn is_global_reference(&self, name: &Name<'_>) -> bool {
        self.host.is_global_reference(name)
    }
}

#[cfg(test)]
mod tests {
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
        with_host(source, |host| {
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
        with_host(source, |host| {
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
    fn only_a_call_statement_has_a_statement_call() {
        let source = "\
f(1).g(2)
local x = h(3)";
        with_host(source, |host| {
            let calls: Vec<Option<usize>> = host
                .nodes
                .iter()
                .filter(|node| matches!(node.kind(), AstKind::SuffixedExpression(_)))
                .map(|node| host.statement_call(node))
                .collect();
            assert_eq!(calls, [Some(2), None]);
        });
    }

    #[test]
    fn every_call_but_a_call_statement_s_last_is_used() {
        let source = "\
f(1).g(2)
local x = h(3)(4)";
        with_host(source, |host| {
            let used: Vec<&[u8]> = host
                .nodes
                .iter()
                .flat_map(|node| host.used_calls(node))
                .map(|call| call.span().text(host.source))
                .collect();
            assert_eq!(used, [&b"f(1)"[..], b"h(3)", b"h(3)(4)"]);
        });
    }

    #[test]
    fn an_assignment_target_reads_every_suffix_but_its_last() {
        let source = "\
a.b.c = x.y.z
t.n += 1";
        with_host(source, |host| {
            let reads: Vec<usize> = host
                .nodes
                .iter()
                .filter(|node| matches!(node.kind(), AstKind::SuffixedExpression(_)))
                .map(|node| host.read_suffixes(node).len())
                .collect();
            assert_eq!(reads, [1, 2, 0]);
        });
    }

    #[test]
    fn a_local_s_declaration_is_where_its_local_assignment_names_it() {
        let source = "\
local a, b = 1
local function f() end";
        with_host(source, |host| {
            let tree = host.scoping();
            let positions: Vec<(&str, Option<usize>)> = tree
                .symbols()
                .iter()
                .map(|symbol| {
                    let position = host.local_declaration(symbol).map(|(_, position)| position);
                    (tree.name(symbol.name), position)
                })
                .collect();
            assert_eq!(positions, [("a", Some(0)), ("b", Some(1)), ("f", None)]);
        });
    }

    #[test]
    fn a_free_name_reads_a_global_and_a_local_doesnt() {
        let source = "\
local x = 1
print(x, y)";
        with_host(source, |host| {
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
