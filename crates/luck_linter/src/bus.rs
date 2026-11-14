use core::cell::RefCell;
use core::ptr;

use luck_ast::{Call, Statement, Suffix};
use luck_semantic::{ChainCallees, SemanticAnalysis};
use rayon::prelude::*;

use crate::diagnostic::Finding;
use crate::node_type::NodeType;
use crate::nodes::{AstNode, NodeKind};
use crate::rule::{CallPosition, CallSite, LintContext, NodeRule};

/// Below this many nodes the chunk setup costs more than it saves.
const MIN_PARALLEL_NODES: usize = 4096;

thread_local! {
    /// For each node type, the indices of the rules subscribed to it. Kept
    /// per thread and reused across files so dispatch doesn't reallocate
    /// the buckets.
    static RULE_BUCKETS: RefCell<Box<[Vec<usize>; NodeType::COUNT]>> =
        RefCell::new(Box::new([const { Vec::new() }; NodeType::COUNT]));
}

/// `call` is the call `node` makes, which goes to `on_call` alone.
fn dispatch(
    rule: &dyn NodeRule,
    node: &AstNode<'_>,
    call: Option<&CallSite<'_>>,
    context: &LintContext<'_>,
    findings: &mut Vec<Finding>,
) {
    if let Some(site) = call {
        rule.on_call(site, context, findings);
        return;
    }
    match node.kind {
        // A call node always has a site, and a last statement is in the table
        // for suppression, with no hook of its own.
        NodeKind::Statement(Statement::FunctionCall(_))
        | NodeKind::Call { .. }
        | NodeKind::LastStatement(_) => {}
        NodeKind::Statement(stmt) => rule.on_stmt(stmt, context, findings),
        NodeKind::Expression(expr) => rule.on_expr(expr, context, findings),
        NodeKind::Table(table) => rule.on_table(table, context, findings),
        NodeKind::Chain(chain, position) => rule.on_chain(chain, position, context, findings),
    }
}

/// The call `node` makes, if it's a call node. `callees` holds the chain
/// of the call before it, so the calls of a chain, which follow each other in
/// the table, resolve in one pass over it.
fn call_site<'ast>(
    node: &AstNode<'ast>,
    callees: &mut Option<ChainCallees<'ast>>,
    semantic: &'ast SemanticAnalysis,
) -> Option<CallSite<'ast>> {
    let (chain, index, position) = match node.kind {
        NodeKind::Statement(Statement::FunctionCall(chain)) => {
            (&**chain, chain.suffixes.len().saturating_sub(1), CallPosition::Statement)
        }
        NodeKind::Call { chain, index } => {
            (chain, usize::try_from(index).ok()?, CallPosition::Expression)
        }
        NodeKind::Statement(_)
        | NodeKind::LastStatement(_)
        | NodeKind::Expression(_)
        | NodeKind::Table(_)
        | NodeKind::Chain(..) => return None,
    };
    let Suffix::Call(suffix) = chain.suffixes.get(index)? else {
        return None;
    };
    if callees.as_ref().is_none_or(|current| !ptr::eq(current.chain(), chain)) {
        *callees = Some(semantic.chain_callees(chain));
    }
    let callee = callees.as_mut().and_then(|current| current.callee(index));
    Some(CallSite { call: Call::new(chain.head(index), suffix), position, callee })
}

/// Large tables are split into contiguous chunks across the rayon pool.
/// Chunk results concatenate in order, so each rule's findings come out
/// as a sequential pass would give them.
pub(crate) fn run<'ast>(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
) -> Vec<Vec<Finding>> {
    if nodes.len() < MIN_PARALLEL_NODES {
        return run_chunk(rules, nodes, context);
    }

    let nodes_per_chunk = nodes.len().div_ceil(rayon::current_num_threads());
    let chunk_results: Vec<Vec<Vec<Finding>>> =
        nodes.par_chunks(nodes_per_chunk).map(|chunk| run_chunk(rules, chunk, context)).collect();

    let mut findings_by_rule = vec![Vec::new(); rules.len()];
    for chunk in chunk_results {
        for (rule_findings, mut chunk_findings) in findings_by_rule.iter_mut().zip(chunk) {
            rule_findings.append(&mut chunk_findings);
        }
    }
    findings_by_rule
}

fn run_chunk<'ast>(
    rules: &[&dyn NodeRule],
    nodes: &[AstNode<'ast>],
    context: &LintContext<'ast>,
) -> Vec<Vec<Finding>> {
    let mut findings_by_rule = vec![Vec::new(); rules.len()];
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

        let mut callees = None;
        #[expect(
            clippy::indexing_slicing,
            reason = "hot loop; every index is in bounds by construction"
        )]
        for node in nodes {
            let slots = &buckets[node.node_type.index()];
            if slots.is_empty() {
                continue;
            }
            let site = call_site(node, &mut callees, context.semantic);
            for &slot in slots {
                dispatch(rules[slot], node, site.as_ref(), context, &mut findings_by_rule[slot]);
            }
        }
    });
    findings_by_rule
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
