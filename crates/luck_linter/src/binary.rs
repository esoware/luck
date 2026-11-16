use luck_ast::{BinaryLink, BinaryOp, Expression};
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
    /// Where [`Self::span`] starts.
    pub first_operand: &'ast Expression,
    /// Where [`Self::span`] ends.
    pub last_operand: &'ast Expression,
}

impl OperatorPair<'_> {
    /// From the start of the left side to the end of the right.
    pub(crate) fn span(&self) -> Span {
        Span::new(self.first_operand.span().start, self.last_operand.span().end)
    }
}

/// Each operator of `run`, the outermost pair first: the last of a
/// left-associative run, and the first of a right-associative one.
pub(crate) fn operator_pairs(run: &BinaryOp) -> impl Iterator<Item = OperatorPair<'_>> {
    let is_left_associative = run.precedence().1 == Associativity::Left;
    let links: &[BinaryLink] = &run.links;
    let count = links.len();
    let run_last_operand = &run.links.last().operand;
    (0..count).filter_map(move |step| {
        let index =
            if is_left_associative { count.saturating_sub(step).saturating_sub(1) } else { step };
        let link = links.get(index)?;
        let left_operand = match index.checked_sub(1) {
            Some(before) => &links.get(before)?.operand,
            None => &run.first,
        };
        Some(if is_left_associative {
            OperatorPair {
                op: link.op,
                lhs: (index == 0).then_some(left_operand),
                rhs: Some(&link.operand),
                first_operand: &run.first,
                last_operand: &link.operand,
            }
        } else {
            OperatorPair {
                op: link.op,
                lhs: Some(left_operand),
                rhs: (index.saturating_add(1) == count).then_some(&link.operand),
                first_operand: left_operand,
                last_operand: run_last_operand,
            }
        })
    })
}
