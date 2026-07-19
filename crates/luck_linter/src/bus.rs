//! Bucketed dispatch for node-local rules.
//!
//! One pass over the flat node table (`LintContext::nodes`) calls each
//! subscribed [`NodeRule`]'s hooks, and only for the node types the rule
//! declares via [`NodeRule::node_types`]. Each rule collects into its own
//! slot so per-rule diagnostic order is identical to the rule walking the
//! table by itself.

use std::cell::RefCell;

use luck_ast::node::{NodeKind, NodeType};
use luck_semantic::nodes::AstNode;

use crate::diagnostic::LintDiagnostic;
use crate::rule::{LintContext, NodeRule};

/// Per-thread rule buckets, reused across files so bucketed dispatch
/// incurs no per-file allocation.
struct RuleBuckets {
    /// `by_type[node_type]` = indices into the per-run rules slice of
    /// rules that subscribed to that node type.
    by_type: Box<[Vec<usize>; NodeType::COUNT]>,
    /// Indices of rules that run on every node (`node_types()` is None).
    any_type: Vec<usize>,
}

impl RuleBuckets {
    fn clear(&mut self) {
        for bucket in self.by_type.iter_mut() {
            bucket.clear();
        }
        self.any_type.clear();
    }
}

thread_local! {
    static RULE_BUCKETS: RefCell<RuleBuckets> = RefCell::new(RuleBuckets {
        by_type: Box::new([const { Vec::new() }; NodeType::COUNT]),
        any_type: Vec::new(),
    });
}

fn dispatch(
    rule: &dyn NodeRule,
    node: &AstNode<'_>,
    ctx: &LintContext,
    out: &mut Vec<LintDiagnostic>,
) {
    match node.kind {
        NodeKind::Statement(stmt) => rule.on_statement(stmt, ctx, out),
        NodeKind::Expression(expr) => rule.on_expression(expr, ctx, out),
    }
}

/// Run `rules` over the node table with type-bucketed dispatch; returns
/// one diagnostics vec per rule, in the same order as `rules`.
///
/// Large tables are split into contiguous chunks dispatched across the
/// rayon pool; per-rule results concatenate in chunk order, so each
/// rule's diagnostic order is identical to a sequential pass.
pub(crate) fn run(rules: &[&dyn NodeRule], ctx: &LintContext) -> Vec<Vec<LintDiagnostic>> {
    let nodes = ctx.nodes.as_slice();
    // Below this size the chunk setup costs more than it saves.
    const MIN_PARALLEL_NODES: usize = 4096;
    if nodes.len() < MIN_PARALLEL_NODES {
        return run_chunk(rules, ctx, nodes);
    }

    use rayon::prelude::*;
    let chunk_len = nodes.len().div_ceil(rayon::current_num_threads());
    let chunk_results: Vec<Vec<Vec<LintDiagnostic>>> = nodes
        .par_chunks(chunk_len)
        .map(|chunk| run_chunk(rules, ctx, chunk))
        .collect();

    let mut out = vec![Vec::new(); rules.len()];
    for chunk in chunk_results {
        for (slot, mut diags) in chunk.into_iter().enumerate() {
            out[slot].append(&mut diags);
        }
    }
    out
}

fn run_chunk(
    rules: &[&dyn NodeRule],
    ctx: &LintContext,
    nodes: &[AstNode<'_>],
) -> Vec<Vec<LintDiagnostic>> {
    let mut out = vec![Vec::new(); rules.len()];
    RULE_BUCKETS.with_borrow_mut(|buckets| {
        buckets.clear();
        for (slot, rule) in rules.iter().enumerate() {
            match rule.node_types() {
                Some(types) => {
                    for bucket_index in 0..NodeType::COUNT {
                        // The bitset is tiny; probing every type keeps
                        // AstTypesBitset free of an iterator API.
                        if types.has_index(bucket_index) {
                            buckets.by_type[bucket_index].push(slot);
                        }
                    }
                }
                None => buckets.any_type.push(slot),
            }
        }

        for node in nodes {
            for &slot in &buckets.by_type[node.node_type as usize] {
                dispatch(rules[slot], node, ctx, &mut out[slot]);
            }
            for &slot in &buckets.any_type {
                dispatch(rules[slot], node, ctx, &mut out[slot]);
            }
        }
    });
    out
}

/// Run one node rule by itself: the `Rule::check` implementation for
/// every converted rule (used directly by per-rule tests).
pub fn run_single(rule: &dyn NodeRule, ctx: &LintContext) -> Vec<LintDiagnostic> {
    run(&[rule], ctx)
        .pop()
        .expect("run returns one slot per rule")
}

/// Reference dispatch: every rule sees every node, ignoring
/// `node_types()` declarations and the driver's file-level skip. The
/// driver compares this against the bucketed path in debug builds.
#[cfg(debug_assertions)]
pub(crate) fn run_every_node(
    rules: &[&dyn NodeRule],
    ctx: &LintContext,
) -> Vec<Vec<LintDiagnostic>> {
    let mut out = vec![Vec::new(); rules.len()];
    for node in ctx.nodes.iter() {
        for (slot, rule) in rules.iter().enumerate() {
            dispatch(*rule, node, ctx, &mut out[slot]);
        }
    }
    out
}
