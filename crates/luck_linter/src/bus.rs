use core::cell::RefCell;
use core::ptr;

use luck_ast::{Call, Statement};
use luck_semantic::{ChainCallees, SemanticAnalysis};
use rayon::prelude::*;

use crate::diagnostic::Finding;
use crate::node_type::NodeType;
use crate::nodes::{AstNode, NodeKind};
use crate::rule::{CallSite, LintContext, NodeRule};

/// Below this many nodes, handing work to the rayon pool costs more than it
/// saves.
pub(crate) const MIN_PARALLEL_NODES: usize = 4096;

/// How many nodes the bus dispatches between asking whether the lint was
/// cancelled.
const NODES_PER_CANCEL_CHECK: usize = 1024;

thread_local! {
    /// For each node type, the indices of the rules subscribed to it. Kept
    /// per thread and reused across files so dispatch doesn't reallocate
    /// the buckets.
    static RULE_BUCKETS: RefCell<Box<[Vec<usize>; NodeType::COUNT]>> =
        RefCell::new(Box::new([const { Vec::new() }; NodeType::COUNT]));
}

/// `call` is the call `node` makes, which goes to `on_call` alone.
fn dispatch<'ast>(
    rule: &dyn NodeRule,
    node: &AstNode<'ast>,
    call: Option<&CallSite<'ast>>,
    context: &LintContext<'ast>,
    findings: &mut Vec<Finding>,
) {
    if let Some(site) = call {
        rule.on_call(site, context, findings);
        return;
    }
    match node.kind {
        // A call node always has a site.
        NodeKind::Statement(Statement::FunctionCall(_)) | NodeKind::Call { .. } => {}
        NodeKind::Statement(stmt) => rule.on_stmt(stmt, context, findings),
        NodeKind::Expression(expr) => rule.on_expr(expr, context, findings),
        NodeKind::Table(table) => rule.on_table(table, context, findings),
        NodeKind::Chain(chain, position) => rule.on_chain(chain, position, context, findings),
    }
}

/// The call `node` makes, if it's a call node. `callees` holds the chain
/// of the call before it, so the calls of a chain, which follow each other in
/// the table front to back, resolve in one pass over it.
fn call_site<'ast>(
    node: &AstNode<'ast>,
    callees: &mut Option<ChainCallees<'ast>>,
    semantic: &'ast SemanticAnalysis<'ast>,
) -> Option<CallSite<'ast>> {
    let (chain, index, position) = node.chain_call()?;
    let call = Call::new(chain.head(index), chain.suffixes.get(index)?)?;
    if callees.as_ref().is_none_or(|current| !ptr::eq(current.chain(), chain)) {
        *callees = Some(semantic.chain_callees(chain));
    }
    let callee = callees.as_mut().and_then(|current| current.callee(index));
    Some(CallSite { call, position, callee })
}

/// Splits `nodes` into contiguous chunks across the rayon pool. Chunk results
/// concatenate in order, so each rule's findings come out as [`run`] gives
/// them.
pub(crate) fn run_in_parallel<'ast>(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<Vec<Finding>>> {
    let nodes_per_chunk = nodes.len().div_ceil(rayon::current_num_threads());
    let chunk_results: Vec<Vec<Vec<Finding>>> = nodes
        .par_chunks(nodes_per_chunk)
        .map(|chunk| run(rules, chunk, context, is_cancelled))
        .collect::<Option<_>>()?;

    let mut findings_by_rule = vec![Vec::new(); rules.len()];
    for chunk in chunk_results {
        for (rule_findings, mut chunk_findings) in findings_by_rule.iter_mut().zip(chunk) {
            rule_findings.append(&mut chunk_findings);
        }
    }
    Some(findings_by_rule)
}

/// Each rule's findings, in node order, or `None` once `is_cancelled` says
/// so.
pub(crate) fn run<'ast>(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<Vec<Finding>>> {
    let mut findings_by_rule = vec![Vec::new(); rules.len()];
    let is_finished = RULE_BUCKETS.with_borrow_mut(|buckets| {
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

        let mut callees = None;
        for nodes_between_checks in nodes.chunks(NODES_PER_CANCEL_CHECK) {
            if is_cancelled() {
                return false;
            }
            #[expect(
                clippy::indexing_slicing,
                reason = "hot loop; every index is in bounds by construction"
            )]
            for node in nodes_between_checks {
                let slots = &buckets[node.node_type.index()];
                if slots.is_empty() {
                    continue;
                }
                let site = call_site(node, &mut callees, context.semantic);
                for &slot in slots {
                    dispatch(
                        rules[slot],
                        node,
                        site.as_ref(),
                        context,
                        &mut findings_by_rule[slot],
                    );
                }
            }
        }
        true
    });
    is_finished.then_some(findings_by_rule)
}

/// Re-runs every node rule on every node and checks that bucketed dispatch
/// and the driver's per-file skipping changed nothing.
///
/// # Panics
///
/// On a mismatch, which means a rule's `node_types()` misses a type its hooks
/// act on.
#[cfg(debug_assertions)]
pub(crate) fn assert_dispatch_is_pure<'ast>(
    dispatched_rules: &[&dyn NodeRule],
    skipped_rules: &[&dyn NodeRule],
    bus_results: &[Vec<Finding>],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
) {
    let reference_results = run_every_node(dispatched_rules, nodes, context);
    for (rule, (bucketed, reference)) in
        dispatched_rules.iter().zip(bus_results.iter().zip(&reference_results))
    {
        assert_eq!(
            bucketed,
            reference,
            "rule `{}`: bucketed dispatch diverged from every-node dispatch; \
             its `node_types()` declaration is missing a type",
            rule.name()
        );
    }
    for (rule, reference) in skipped_rules.iter().zip(run_every_node(skipped_rules, nodes, context))
    {
        assert!(
            reference.is_empty(),
            "rule `{}` was skipped for this file but produces findings; \
             its `node_types()` declaration is missing a type",
            rule.name()
        );
    }
}

#[cfg(debug_assertions)]
fn run_every_node<'ast>(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
) -> Vec<Vec<Finding>> {
    let mut findings_by_rule = vec![Vec::new(); rules.len()];
    let mut callees = None;
    for node in nodes {
        let site = call_site(node, &mut callees, context.semantic);
        for (rule, rule_findings) in rules.iter().zip(&mut findings_by_rule) {
            dispatch(*rule, node, site.as_ref(), context, rule_findings);
        }
    }
    findings_by_rule
}
