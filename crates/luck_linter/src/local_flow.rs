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
            walk.mark_next_uses(&uses, local);
            if !is_uninitialized && walk.is_overwritten_unread(graph.successors(local.node)) {
                self.overwritten_values.push((symbol.id, symbol.definition_span));
            }
            for (i, used) in uses.iter().enumerate() {
                if !used.kind.is_write() {
                    continue;
                }
                let next_in_node =
                    uses.get(i.saturating_add(1)).filter(|next| next.node == used.node);
                let is_overwritten = next_in_node.map_or_else(
                    || walk.is_overwritten_unread(graph.successors(used.node)),
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

/// A set of nodes that clears in constant time, so a long function doesn't
/// allocate a fresh array for each local.
struct NodeSet {
    stamps: Vec<u32>,
    current: u32,
}

impl NodeSet {
    fn new(node_count: usize) -> Self {
        Self { stamps: vec![0; node_count], current: 1 }
    }

    const fn clear(&mut self) {
        self.current = self.current.saturating_add(1);
    }

    fn insert(&mut self, node: usize) -> bool {
        let current = self.current;
        self.stamps.get_mut(node).is_some_and(|stamp| mem::replace(stamp, current) != current)
    }

    fn contains(&self, node: usize) -> bool {
        self.stamps.get(node) == Some(&self.current)
    }
}

/// Past the local's scope or at a re-run of its `local` statement, which
/// replaces the variable, its value can't go on.
fn is_alive_at(graph: &FlowGraph, node: usize, local: &LocalDeclaration) -> bool {
    node != local.node && !graph.starts_at_or_after(node, local.scope_end)
}

struct GraphWalk<'graph> {
    graph: &'graph FlowGraph,
    /// Each edge as `(successor, predecessor)`, sorted.
    edges_by_successor: Vec<(usize, usize)>,
    visited: NodeSet,
    used: NodeSet,
    reads_ahead: NodeSet,
    writes_ahead: NodeSet,
    stack: Vec<usize>,
}

impl<'graph> GraphWalk<'graph> {
    fn new(graph: &'graph FlowGraph) -> Self {
        let node_count = graph.node_count();
        let mut edges_by_successor: Vec<(usize, usize)> = (0..node_count)
            .flat_map(|node| graph.successors(node).map(move |successor| (successor, node)))
            .collect();
        edges_by_successor.sort_unstable();
        Self {
            graph,
            edges_by_successor,
            visited: NodeSet::new(node_count),
            used: NodeSet::new(node_count),
            reads_ahead: NodeSet::new(node_count),
            writes_ahead: NodeSet::new(node_count),
            stack: Vec::new(),
        }
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
        self.visited.clear();
        self.stack.clear();
        self.stack.extend(
            uses.iter()
                .filter(|used| used.kind.is_write())
                .flat_map(|used| graph.successors(used.node)),
        );
        while let Some(node) = self.stack.pop() {
            if is_alive_at(graph, node, local) && self.visited.insert(node) {
                self.stack.extend(graph.successors(node));
            }
        }
        for used in uses {
            if used.kind.is_read() && !used.is_in_closure && !self.visited.contains(used.node) {
                return Some(used.reference_id);
            }
            // A later read in the statement that wrote sees the value.
            if used.kind.is_write() {
                self.visited.insert(used.node);
            }
        }
        None
    }

    /// Marks each node from which some path's next use of the local is a read
    /// or a write. One backward pass per local, because walking forward from
    /// every write repeats the search once per assignment. `uses` is sorted by
    /// node, each node's in reference order.
    fn mark_next_uses(&mut self, uses: &[Use], local: &LocalDeclaration) {
        let Self { graph, edges_by_successor, used, reads_ahead, writes_ahead, stack, .. } = self;
        used.clear();
        reads_ahead.clear();
        writes_ahead.clear();
        for found in uses {
            used.insert(found.node);
        }
        for first in uses.chunk_by(|a, b| a.node == b.node).filter_map(<[Use]>::first) {
            let next_use =
                if first.kind.is_read() { &mut *reads_ahead } else { &mut *writes_ahead };
            stack.push(first.node);
            while let Some(node) = stack.pop() {
                if is_alive_at(graph, node, local) && next_use.insert(node) {
                    let first_edge =
                        edges_by_successor.partition_point(|&(successor, _)| successor < node);
                    stack.extend(
                        edges_by_successor
                            .get(first_edge..)
                            .unwrap_or_default()
                            .iter()
                            .take_while(|&&(successor, _)| successor == node)
                            .map(|&(_, predecessor)| predecessor)
                            .filter(|&predecessor| !used.contains(predecessor)),
                    );
                }
            }
        }
    }

    /// Whether a value leaving through `starts` is overwritten on some path
    /// and read on none.
    fn is_overwritten_unread(&self, starts: impl IntoIterator<Item = usize>) -> bool {
        let mut is_overwritten = false;
        for start in starts {
            if self.reads_ahead.contains(start) {
                return false;
            }
            is_overwritten = is_overwritten || self.writes_ahead.contains(start);
        }
        is_overwritten
    }
}
