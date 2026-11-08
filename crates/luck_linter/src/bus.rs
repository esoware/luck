//! Bucketed dispatch for node rules.
//!
//! One pass over the flat node table calls each [`NodeRule`]'s hooks for
//! the node types it declares in [`NodeRule::node_types`]. Each rule
//! collects into its own list, so its diagnostics come out in the order a
//! walk of its own would give.

use core::cell::RefCell;

use luck_ast::{NodeKind, NodeType};
use rayon::prelude::*;

use crate::diagnostic::LintDiagnostic;
use crate::nodes::AstNode;
use crate::rule::{LintContext, NodeRule};

/// Below this many nodes the chunk setup costs more than it saves.
const MIN_PARALLEL_NODES: usize = 4096;

thread_local! {
    /// For each node type, the indices of the rules subscribed to it. Kept
    /// per thread and reused across files so dispatch doesn't reallocate
    /// the buckets.
    static RULE_BUCKETS: RefCell<Box<[Vec<usize>; NodeType::COUNT]>> =
        RefCell::new(Box::new([const { Vec::new() }; NodeType::COUNT]));
}

fn dispatch(
    rule: &dyn NodeRule,
    node: &AstNode<'_>,
    context: &LintContext<'_>,
    diags: &mut Vec<LintDiagnostic>,
) {
    match node.kind {
        NodeKind::Statement(stmt) => rule.on_statement(stmt, context, diags),
        NodeKind::LastStatement(last_stmt) => rule.on_last_statement(last_stmt, context, diags),
        NodeKind::Expression(expr) => rule.on_expression(expr, context, diags),
    }
}

/// Runs `rules` over `nodes` and returns one diagnostics list per rule, in
/// the order of `rules`.
///
/// Large tables are split into contiguous chunks across the rayon pool.
/// Chunk results concatenate in order, so each rule's diagnostics come out
/// as a sequential pass would give them.
pub(crate) fn run(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'_>],
    context: &LintContext<'_>,
) -> Vec<Vec<LintDiagnostic>> {
    if nodes.len() < MIN_PARALLEL_NODES {
        return run_chunk(rules, nodes, context);
    }

    let nodes_per_chunk = nodes.len().div_ceil(rayon::current_num_threads());
    let chunk_results: Vec<Vec<Vec<LintDiagnostic>>> =
        nodes.par_chunks(nodes_per_chunk).map(|chunk| run_chunk(rules, chunk, context)).collect();

    let mut diags_by_rule = vec![Vec::new(); rules.len()];
    for chunk in chunk_results {
        for (rule_diags, mut chunk_diags) in diags_by_rule.iter_mut().zip(chunk) {
            rule_diags.append(&mut chunk_diags);
        }
    }
    diags_by_rule
}

fn run_chunk(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'_>],
    context: &LintContext<'_>,
) -> Vec<Vec<LintDiagnostic>> {
    let mut diags_by_rule = vec![Vec::new(); rules.len()];
    RULE_BUCKETS.with_borrow_mut(|buckets| {
        for bucket in buckets.iter_mut() {
            bucket.clear();
        }
        for (slot, rule) in rules.iter().enumerate() {
            let node_types = rule.node_types();
            for (bucket_index, bucket) in buckets.iter_mut().enumerate() {
                // The bitset is tiny; probing every type keeps NodeTypeSet
                // free of an iterator API.
                if node_types.has_index(bucket_index) {
                    bucket.push(slot);
                }
            }
        }

        #[expect(
            clippy::indexing_slicing,
            reason = "hot loop; every index is in bounds by construction"
        )]
        for node in nodes {
            for &slot in &buckets[node.node_type.index()] {
                dispatch(rules[slot], node, context, &mut diags_by_rule[slot]);
            }
        }
    });
    diags_by_rule
}

/// Re-runs every node rule on every node and checks that bucketed dispatch
/// and the driver's per-file skipping changed nothing.
///
/// # Panics
///
/// On a mismatch, which means a rule's `node_types()` misses a type its hooks
/// act on.
#[cfg(debug_assertions)]
pub(crate) fn assert_dispatch_is_pure(
    dispatched_rules: &[&dyn NodeRule],
    skipped_rules: &[&dyn NodeRule],
    bus_results: &[Vec<LintDiagnostic>],
    nodes: &[AstNode<'_>],
    context: &LintContext<'_>,
) {
    let reference_results = run_every_node(dispatched_rules, nodes, context);
    for (rule, (bucketed, reference)) in
        dispatched_rules.iter().zip(bus_results.iter().zip(&reference_results))
    {
        assert_eq!(
            bucketed,
            reference,
            "rule '{}': bucketed dispatch diverged from every-node dispatch; \
             its node_types() declaration is missing a type",
            rule.name()
        );
    }
    for (rule, reference) in skipped_rules.iter().zip(run_every_node(skipped_rules, nodes, context))
    {
        assert!(
            reference.is_empty(),
            "rule '{}' was skipped for this file but produces diagnostics; \
             its node_types() declaration is missing a type",
            rule.name()
        );
    }
}

/// Runs every rule on every node, ignoring `node_types()`.
#[cfg(debug_assertions)]
fn run_every_node(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'_>],
    context: &LintContext<'_>,
) -> Vec<Vec<LintDiagnostic>> {
    let mut diags_by_rule = vec![Vec::new(); rules.len()];
    for node in nodes {
        for (rule, rule_diags) in rules.iter().zip(&mut diags_by_rule) {
            dispatch(*rule, node, context, rule_diags);
        }
    }
    diags_by_rule
}
