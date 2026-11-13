use luck_ast::{BinaryOp, Expression};
use luck_token::{Associativity, BinOp, Span};

/// One operator of a run, with what it applies to in the pairs the run
/// groups into.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OperatorPair<'ast> {
    pub op: BinOp,
    /// `None` when the left side is the operators of the run before this one.
    pub lhs: Option<&'ast Expression>,
    /// `None` when the right side is the operators of the run after this one.
    pub rhs: Option<&'ast Expression>,
    /// From the start of the left side to the end of the right.
    pub span: Span,
}

/// Each operator of `run`, the outermost pair first: the last of a
/// left-associative run, and the first of a right-associative one.
pub(crate) fn operator_pairs(run: &BinaryOp) -> impl Iterator<Item = OperatorPair<'_>> {
    let is_left_associative = run.precedence().1 == Associativity::Left;
    let count = run.links.len();
    let run_start = run.first.span().start;
    let run_end = run.links.last().operand.span().end;
    (0..count).filter_map(move |step| {
        let index =
            if is_left_associative { count.saturating_sub(step).saturating_sub(1) } else { step };
        let link = run.links.get(index)?;
        let left_operand = match index.checked_sub(1) {
            Some(before) => &run.links.get(before)?.operand,
            None => &run.first,
        };
        Some(if is_left_associative {
            OperatorPair {
                op: link.op,
                lhs: (index == 0).then_some(left_operand),
                rhs: Some(&link.operand),
                span: Span::new(run_start, link.operand.span().end),
            }
        } else {
            OperatorPair {
                op: link.op,
                lhs: Some(left_operand),
                rhs: (index.saturating_add(1) == count).then_some(&link.operand),
                span: Span::new(left_operand.span().start, run_end),
            }
        })
    })
}
