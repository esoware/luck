use core::mem;

use luck_semantic::{ReferenceId, ReferenceKind, Symbol, SymbolId, SymbolKind};
use luck_token::Span;

use crate::cfg::{FlowGraph, LocalDeclaration};
use crate::rule::LintContext;

#[derive(Debug, Default)]
pub(crate) struct LocalFlow {
    /// Each local declared without a value, with its first use that no
    /// assignment can reach.
    pub first_unassigned_uses: Vec<(SymbolId, ReferenceId)>,
    /// Each local, with where a value is written to it that some path
    /// overwrites and no path reads.
    pub overwritten_values: Vec<(SymbolId, Span)>,
}

impl LocalFlow {
    pub(crate) fn compute(context: &LintContext<'_>) -> Self {
        let tree = &context.semantic.scope_tree;
        let mut tracked: Vec<&Symbol> = tree
            .symbols
            .iter()
            .filter(|symbol| {
                !symbol.name.starts_with('_')
                    && (context.uninitialized_locals.contains(symbol)
                        || (symbol.kind == SymbolKind::Local
                            && symbol.is_modified
                            && !symbol.is_upvalue))
            })
            .collect();
        tracked.sort_unstable_by_key(|symbol| symbol.definition_span.start);

        let mut flow = Self::default();
        for &function_block in context.function_blocks {
            let span = function_block.span;
            let first_inside =
                tracked.partition_point(|symbol| symbol.definition_span.start < span.start);
            if tracked
                .get(first_inside)
                .is_none_or(|symbol| symbol.definition_span.start >= span.end)
            {
                continue;
            }
            let graph = FlowGraph::of_block(function_block, context.semantic);
            let mut walk = GraphWalk::new(&graph);
            for local in graph.locals() {
                if let Ok(index) = tracked.binary_search_by_key(&local.name_span.start, |symbol| {
                    symbol.definition_span.start
                }) && let Some(symbol) = tracked.get(index)
                {
                    flow.add_local(symbol, local, &mut walk, context);
                }
            }
        }
        flow
    }

    fn add_local(
        &mut self,
        symbol: &Symbol,
        local: &LocalDeclaration,
        walk: &mut GraphWalk<'_>,
        context: &LintContext<'_>,
    ) {
        let tree = &context.semantic.scope_tree;
        let graph = walk.graph;
        let mut uses = Vec::with_capacity(symbol.reference_ids.len());
        for &reference_id in &symbol.reference_ids {
            let reference = tree.reference(reference_id);
            let Some(node) = graph.node_at(reference.span.start) else {
                return;
            };
            uses.push(Use {
                node,
                reference_id,
                span: reference.span,
                kind: reference.kind,
                is_in_closure: symbol.is_upvalue
                    && tree.crosses_function_boundary(reference.scope, symbol.scope),
            });
        }

        let is_uninitialized = context.uninitialized_locals.contains(symbol);
        if is_uninitialized && let Some(reference_id) = walk.first_unassigned_use(&uses, local) {
            self.first_unassigned_uses.push((symbol.id, reference_id));
        }

        // A closure can read a captured local at any time.
        if symbol.is_modified && !symbol.is_upvalue {
            // Stable, so each node's uses stay in reference order.
            uses.sort_by_key(|used| used.node);
            if !is_uninitialized
                && walk.is_overwritten_unread(&uses, local, graph.successors(local.node))
            {
                self.overwritten_values.push((symbol.id, symbol.definition_span));
            }
            for (i, used) in uses.iter().enumerate() {
                if !used.kind.is_write() {
                    continue;
                }
                let next_in_node =
                    uses.get(i.saturating_add(1)).filter(|next| next.node == used.node);
                let is_overwritten = next_in_node.map_or_else(
                    || walk.is_overwritten_unread(&uses, local, graph.successors(used.node)),
                    |next| !next.kind.is_read(),
                );
                if is_overwritten {
                    self.overwritten_values.push((symbol.id, used.span));
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Use {
    node: usize,
    reference_id: ReferenceId,
    span: Span,
    kind: ReferenceKind,
    is_in_closure: bool,
}

/// Follows a local's value along a graph's paths, stopping where the value
/// can't go: past the local's scope, or at a re-run of its `local` statement,
/// which replaces the variable.
///
/// The marks are shared across locals, so a long function doesn't allocate a
/// fresh array for each one.
struct GraphWalk<'graph> {
    graph: &'graph FlowGraph,
    visited_in_walk: Vec<u32>,
    walk: u32,
    stack: Vec<usize>,
}

impl<'graph> GraphWalk<'graph> {
    fn new(graph: &'graph FlowGraph) -> Self {
        Self { graph, visited_in_walk: vec![0; graph.node_count()], walk: 0, stack: Vec::new() }
    }

    /// The first read, in reference order, that no write can reach. A read
    /// in a closure runs at an unknown time, and a write in a closure can
    /// happen anywhere after the closure exists.
    fn first_unassigned_use(
        &mut self,
        uses: &[Use],
        local: &LocalDeclaration,
    ) -> Option<ReferenceId> {
        let graph = self.graph;
        self.start(
            uses.iter()
                .filter(|used| used.kind.is_write())
                .flat_map(|used| graph.successors(used.node)),
        );
        while let Some(node) = self.next_node(local) {
            self.stack.extend(graph.successors(node));
        }
        for used in uses {
            if used.kind.is_read() && !used.is_in_closure && !self.is_visited(used.node) {
                return Some(used.reference_id);
            }
            // A later read in the statement that wrote sees the value.
            if used.kind.is_write() {
                self.visit(used.node);
            }
        }
        None
    }

    /// Whether a value leaving through `starts` is overwritten on some path
    /// and read on none. `uses` is sorted by node, each node's in reference
    /// order.
    fn is_overwritten_unread(
        &mut self,
        uses: &[Use],
        local: &LocalDeclaration,
        starts: impl IntoIterator<Item = usize>,
    ) -> bool {
        self.start(starts);
        let mut is_overwritten = false;
        while let Some(node) = self.next_node(local) {
            let first_use = uses.partition_point(|candidate| candidate.node < node);
            match uses.get(first_use).filter(|candidate| candidate.node == node) {
                Some(found) if found.kind.is_read() => return false,
                Some(_) => is_overwritten = true,
                None => self.stack.extend(self.graph.successors(node)),
            }
        }
        is_overwritten
    }

    fn start(&mut self, starts: impl IntoIterator<Item = usize>) {
        self.walk = self.walk.saturating_add(1);
        self.stack.clear();
        self.stack.extend(starts);
    }

    fn next_node(&mut self, local: &LocalDeclaration) -> Option<usize> {
        while let Some(node) = self.stack.pop() {
            if node != local.node
                && !self.graph.starts_at_or_after(node, local.scope_end)
                && self.visit(node)
            {
                return Some(node);
            }
        }
        None
    }

    fn visit(&mut self, node: usize) -> bool {
        self.visited_in_walk
            .get_mut(node)
            .is_some_and(|visited| mem::replace(visited, self.walk) != self.walk)
    }

    fn is_visited(&self, node: usize) -> bool {
        self.visited_in_walk.get(node) == Some(&self.walk)
    }
}
