//! What each function's control flow says about its locals, computed once
//! per chunk for the rules that follow a local's value.

use core::mem;

use luck_semantic::{ReferenceId, ReferenceKind, Symbol, SymbolId, SymbolKind};
use luck_token::Span;

use crate::cfg::FlowGraph;
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
            for &(name_span, declaration) in graph.locals() {
                if let Ok(index) = tracked
                    .binary_search_by_key(&name_span.start, |symbol| symbol.definition_span.start)
                    && let Some(symbol) = tracked.get(index)
                {
                    flow.add_local(symbol, declaration, &graph, context);
                }
            }
        }
        flow
    }

    fn add_local(
        &mut self,
        symbol: &Symbol,
        declaration: usize,
        graph: &FlowGraph,
        context: &LintContext<'_>,
    ) {
        let tree = &context.semantic.scope_tree;
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
        if is_uninitialized
            && let Some(reference_id) = first_unassigned_use(&uses, declaration, graph)
        {
            self.first_unassigned_uses.push((symbol.id, reference_id));
        }

        // A closure can read a captured local at any time.
        if symbol.is_modified && !symbol.is_upvalue {
            // Stable, so each node's uses stay in reference order.
            uses.sort_by_key(|used| used.node);
            let mut value_walk = ValueWalk::new(graph, &uses, declaration);
            if !is_uninitialized && value_walk.is_overwritten_unread(graph.successors(declaration))
            {
                self.overwritten_values.push((symbol.id, symbol.definition_span));
            }
            for (index, used) in uses.iter().enumerate() {
                if !used.kind.is_write() {
                    continue;
                }
                let next_in_node =
                    uses.get(index.saturating_add(1)).filter(|next| next.node == used.node);
                let is_overwritten = next_in_node.map_or_else(
                    || value_walk.is_overwritten_unread(graph.successors(used.node)),
                    |next| !next.kind.is_read(),
                );
                if is_overwritten {
                    self.overwritten_values.push((symbol.id, used.span));
                }
            }
        }
    }
}

/// One reference to a tracked local, from its own function or a closure.
#[derive(Debug, Clone, Copy)]
struct Use {
    node: usize,
    reference_id: ReferenceId,
    span: Span,
    kind: ReferenceKind,
    is_in_closure: bool,
}

/// The first read, in reference order, that no write can reach. A read in a
/// closure runs at an unknown time, and a write in a closure can happen
/// anywhere after the closure exists.
fn first_unassigned_use(
    uses: &[Use],
    declaration: usize,
    graph: &FlowGraph,
) -> Option<ReferenceId> {
    let mut is_assigned = vec![false; graph.node_count()];
    let after_writes = uses
        .iter()
        .filter(|used| used.kind.is_write())
        .flat_map(|used| graph.successors(used.node));
    graph.mark_reachable_from(after_writes, Some(declaration), &mut is_assigned);
    for used in uses {
        if used.kind.is_read()
            && !used.is_in_closure
            && !is_assigned.get(used.node).copied().unwrap_or_default()
        {
            return Some(used.reference_id);
        }
        // A later read in the statement that wrote sees the value.
        if used.kind.is_write()
            && let Some(mark) = is_assigned.get_mut(used.node)
        {
            *mark = true;
        }
    }
    None
}

/// Follows a value along every path out of a write, until something reads
/// it, overwrites it, or a re-run of its `local` statement replaces the
/// variable.
struct ValueWalk<'walk> {
    graph: &'walk FlowGraph,
    /// Sorted by node, each node's uses in reference order.
    uses: &'walk [Use],
    declaration: usize,
    /// The last walk that visited each node.
    visited_in_walk: Vec<u32>,
    walk: u32,
    stack: Vec<usize>,
}

impl<'walk> ValueWalk<'walk> {
    fn new(graph: &'walk FlowGraph, uses: &'walk [Use], declaration: usize) -> Self {
        Self {
            graph,
            uses,
            declaration,
            visited_in_walk: vec![0; graph.node_count()],
            walk: 0,
            stack: Vec::new(),
        }
    }

    /// Whether a value leaving through `starts` is overwritten on some path
    /// and read on none.
    fn is_overwritten_unread(&mut self, starts: impl IntoIterator<Item = usize>) -> bool {
        self.walk = self.walk.saturating_add(1);
        self.stack.clear();
        self.stack.extend(starts);
        let mut is_overwritten = false;
        while let Some(node) = self.stack.pop() {
            if node == self.declaration || !self.visit(node) {
                continue;
            }
            match self.first_use_in(node) {
                Some(used) if used.kind.is_read() => return false,
                Some(_) => is_overwritten = true,
                None => self.stack.extend(self.graph.successors(node)),
            }
        }
        is_overwritten
    }

    /// Records this walk's visit to `node`, false if it's already been.
    fn visit(&mut self, node: usize) -> bool {
        self.visited_in_walk
            .get_mut(node)
            .is_some_and(|visited| mem::replace(visited, self.walk) != self.walk)
    }

    fn first_use_in(&self, node: usize) -> Option<&Use> {
        let index = self.uses.partition_point(|used| used.node < node);
        self.uses.get(index).filter(|used| used.node == node)
    }
}
