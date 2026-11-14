use core::mem;

use luck_token::{Associativity, BinOp, OperandSide, Span, Token, TokenKind};

use crate::expr::{
    BinaryLink, BinaryOp, CallSuffix, Expression, FunctionArgs, Suffix, SuffixedExpression,
};
use crate::one_or_more::OneOrMore;
use crate::query::DottedPath;
use crate::types::TypeArgs;

/// A chain's prefix and its first few suffixes, read as one expression: what
/// a call calls, a method's receiver, or what a field or index reads from.
#[derive(Debug, Clone, Copy)]
pub struct ChainHead<'ast> {
    pub prefix: &'ast Expression,
    pub suffixes: &'ast [Suffix],
}

impl<'ast> ChainHead<'ast> {
    /// The prefix, when no suffix follows it.
    #[must_use]
    pub const fn as_expr(self) -> Option<&'ast Expression> {
        if self.suffixes.is_empty() { Some(self.prefix) } else { None }
    }

    /// The name, when the head is nothing else.
    #[must_use]
    pub const fn as_name(self) -> Option<&'ast Token> {
        if let Some(Expression::Name(token)) = self.as_expr() { Some(token) } else { None }
    }

    /// `None` for a prefix other than a name, or a suffix other than a
    /// field.
    #[must_use]
    pub fn dotted_path(self) -> Option<DottedPath<'ast>> {
        let Expression::Name(root) = self.prefix else {
            return None;
        };
        let mut segments = Vec::with_capacity(self.suffixes.len().saturating_add(1));
        segments.push(root.identifier()?);
        let mut leaf_span = root.span;
        for suffix in self.suffixes {
            let Suffix::Field(name) = suffix else {
                return None;
            };
            segments.push(name.identifier()?);
            leaf_span = name.span;
        }
        Some(DottedPath { segments, root_span: root.span, leaf_span })
    }

    #[must_use]
    pub fn split_last(self) -> Option<(Self, &'ast Suffix)> {
        let (last, before) = self.suffixes.split_last()?;
        Some((Self { prefix: self.prefix, suffixes: before }, last))
    }

    #[must_use]
    pub fn span(self) -> Span {
        let prefix_span = self.prefix.span();
        self.suffixes.last().map_or(prefix_span, |last| prefix_span.merge(last.span()))
    }
}

/// A suffixed expression is its whole chain, and any other expression is a
/// prefix with no suffixes.
impl<'ast> From<&'ast Expression> for ChainHead<'ast> {
    fn from(expr: &'ast Expression) -> Self {
        if let Expression::Suffixed(chain) = expr {
            Self { prefix: &chain.prefix, suffixes: &chain.suffixes }
        } else {
            Self { prefix: expr, suffixes: &[] }
        }
    }
}

/// A call in a [`SuffixedExpression`], and the part of the chain before it.
#[derive(Debug, Clone, Copy)]
pub struct Call<'ast> {
    /// The receiver, for a method call.
    pub callee: ChainHead<'ast>,
    pub method: Option<&'ast Token>,
    /// See [`CallSuffix::explicit_type_args`].
    pub explicit_type_args: Option<&'ast TypeArgs>,
    pub args: &'ast FunctionArgs,
}

impl<'ast> Call<'ast> {
    #[must_use]
    pub fn new(callee: ChainHead<'ast>, suffix: &'ast CallSuffix) -> Self {
        Self {
            callee,
            method: suffix.method.as_ref(),
            explicit_type_args: suffix.explicit_type_args.as_deref(),
            args: &suffix.args,
        }
    }

    #[must_use]
    pub fn span(self) -> Span {
        self.callee.span().merge(self.args.span())
    }
}

impl SuffixedExpression {
    /// The prefix and the first `count` suffixes, or all of them when there
    /// are fewer.
    #[must_use]
    pub fn head(&self, count: usize) -> ChainHead<'_> {
        let suffixes: &[Suffix] = &self.suffixes;
        ChainHead { prefix: &self.prefix, suffixes: suffixes.get(..count).unwrap_or(suffixes) }
    }

    /// Every call in the chain, first to last.
    pub fn calls(&self) -> impl Iterator<Item = Call<'_>> {
        self.suffixes.iter().enumerate().filter_map(|(i, suffix)| {
            if let Suffix::Call(call) = suffix { Some(Call::new(self.head(i), call)) } else { None }
        })
    }

    /// The call the chain ends in, if it ends in one.
    #[must_use]
    pub fn last_call(&self) -> Option<Call<'_>> {
        let (last, before) = self.suffixes.split_last()?;
        if let Suffix::Call(call) = last {
            Some(Call::new(ChainHead { prefix: &self.prefix, suffixes: before }, call))
        } else {
            None
        }
    }

    /// Whether the chain ends in a call, which makes it one too.
    #[must_use]
    pub fn is_call(&self) -> bool {
        matches!(self.suffixes.last(), Suffix::Call(_))
    }

    /// Whether the chain ends in a field or an index, which makes it a
    /// [`Var`](crate::Var).
    #[must_use]
    pub fn is_var(&self) -> bool {
        matches!(self.suffixes.last(), Suffix::Field(_) | Suffix::Index { .. })
    }

    /// Merges a suffixed prefix, which a rewrite may leave, into this run, so
    /// the tree nests no deeper than the parser would read it. A prefix
    /// nested any number of runs deep is merged in one pass over the
    /// suffixes it moves.
    pub fn splice_prefix(&mut self) {
        if !matches!(self.prefix, Expression::Suffixed(_)) {
            return;
        }
        let mut fronts = Vec::new();
        let mut prefix = self.prefix.take();
        while let Expression::Suffixed(inner) = prefix {
            let Self { prefix: inner_prefix, suffixes, .. } = *inner;
            fronts.push(suffixes);
            prefix = inner_prefix;
        }
        self.prefix = prefix;
        let front_count = fronts.iter().map(|front| front.len()).sum::<usize>();
        let mut suffixes = Vec::with_capacity(front_count.saturating_add(self.suffixes.len()));
        for front in fronts.into_iter().rev() {
            suffixes.extend(front.into_vec());
        }
        let placeholder = Suffix::Field(Token::new(TokenKind::Eof, Span::default()));
        suffixes.extend(mem::replace(&mut self.suffixes, OneOrMore::new(placeholder)).into_vec());
        if let Some(spliced) = OneOrMore::from_vec(suffixes) {
            self.suffixes = spliced;
        }
    }
}

impl BinaryOp {
    /// What every operator in the run shares.
    #[must_use]
    pub fn precedence(&self) -> (u8, Associativity) {
        self.links.first().op.precedence()
    }

    /// See [`BinOp::can_nest_bare`].
    #[must_use]
    pub fn can_nest_bare(&self, outer: BinOp, side: OperandSide) -> bool {
        self.links.first().op.can_nest_bare(outer, side)
    }

    /// Merges each operand that a rewrite left as a bare run at this level
    /// into this run, as printing and reading it back would, so the tree
    /// nests no deeper than the parser would read it.
    ///
    /// A bare run among a merged run's own operands merges too, however many
    /// runs deep, in one pass over the operands that move.
    pub fn splice_operands(&mut self) {
        let level = self.precedence().0;
        let is_bare_run = |operand: &Expression| matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 == level);
        if !is_bare_run(&self.first) && !self.links.iter().any(|link| is_bare_run(&link.operand)) {
            return;
        }
        let placeholder = BinaryLink { op: BinOp::Or, operand: Expression::Error(Span::default()) };
        let old_links = mem::replace(&mut self.links, OneOrMore::new(placeholder));
        let mut links = Vec::with_capacity(old_links.len());
        // The operands still to place, innermost run's on top, and the next
        // one with the operator before it, which the first operand lacks.
        let mut pending = vec![old_links.into_vec().into_iter()];
        let mut next = Some((None, self.first.take()));
        while let Some((leading_op, operand)) = next.take() {
            if is_bare_run(&operand) {
                if let Expression::BinaryOp(inner) = operand {
                    let Self { first, links: inner_links, .. } = *inner;
                    pending.push(inner_links.into_vec().into_iter());
                    next = Some((leading_op, first));
                }
                continue;
            }
            match leading_op {
                None => self.first = operand,
                Some(op) => links.push(BinaryLink { op, operand }),
            }
            while let Some(operands) = pending.last_mut() {
                if let Some(link) = operands.next() {
                    next = Some((Some(link.op), link.operand));
                    break;
                }
                pending.pop();
            }
        }
        if let Some(spliced) = OneOrMore::from_vec(links) {
            self.links = spliced;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;

    fn unspliced_chain(prefix: Expression, suffix: Suffix) -> Expression {
        Expression::Suffixed(Box::new(SuffixedExpression {
            span: Span::default(),
            prefix,
            suffixes: OneOrMore::new(suffix),
        }))
    }

    fn unspliced_run(first: Expression, op: BinOp, operand: Expression) -> Expression {
        Expression::BinaryOp(Box::new(BinaryOp {
            span: Span::default(),
            first,
            links: OneOrMore::new(BinaryLink { op, operand }),
        }))
    }

    #[test]
    fn splice_prefix_joins_every_suffixed_prefix_in_order() {
        let synth = Synth::new();
        let called = synth.call(synth.name_expr("f"), Vec::new());
        let field = unspliced_chain(called, Suffix::Field(synth.identifier("a")));
        let Expression::Suffixed(mut chain) =
            unspliced_chain(field, Suffix::Field(synth.identifier("b")))
        else {
            panic!("expected a suffixed expression");
        };
        chain.splice_prefix();
        assert!(matches!(chain.prefix, Expression::Name(_)));
        assert!(matches!(*chain.suffixes, [Suffix::Call(_), Suffix::Field(_), Suffix::Field(_)]));
    }

    #[test]
    fn splice_operands_joins_bare_runs_at_its_level_in_order() {
        let synth = Synth::new();
        let name = |name: &str| synth.name_expr(name);
        // (a + b) - (c * d + e), with no parentheses: a + b - c * d + e.
        let left = unspliced_run(name("a"), BinOp::Add, name("b"));
        let product = unspliced_run(name("c"), BinOp::Mul, name("d"));
        let right = unspliced_run(product, BinOp::Add, name("e"));
        let Expression::BinaryOp(mut run) = unspliced_run(left, BinOp::Sub, right) else {
            panic!("expected a binary run");
        };
        run.splice_operands();
        let ops: Vec<BinOp> = run.links.iter().map(|link| link.op).collect();
        assert_eq!(ops, [BinOp::Add, BinOp::Sub, BinOp::Add]);
        assert!(matches!(run.first, Expression::Name(_)));
        assert!(matches!(run.links[1].operand, Expression::BinaryOp(_)), "`c * d` stays a run");
    }

    #[test]
    fn splicing_joins_runs_nested_any_number_of_levels_deep() {
        let synth = Synth::new();
        let name = || synth.name_expr("x");
        let mut sum = name();
        let mut concat = name();
        let mut path = name();
        for _ in 0..50_000u32 {
            sum = unspliced_run(sum, BinOp::Add, name());
            concat = unspliced_run(name(), BinOp::Concat, concat);
            path = unspliced_chain(path, Suffix::Field(synth.identifier("a")));
        }
        for nested in [sum, concat] {
            let Expression::BinaryOp(mut run) = nested else {
                panic!("expected a binary run");
            };
            run.splice_operands();
            assert_eq!(run.links.len(), 50_000);
            assert!(matches!(run.first, Expression::Name(_)));
            assert!(run.links.iter().all(|link| matches!(link.operand, Expression::Name(_))));
        }
        let Expression::Suffixed(mut chain) = path else {
            panic!("expected a suffixed expression");
        };
        chain.splice_prefix();
        assert_eq!(chain.suffixes.len(), 50_000);
        assert!(matches!(chain.prefix, Expression::Name(_)));
    }
}
