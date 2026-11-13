use core::mem;

use luck_token::{Associativity, BinOp, OperandSide, Span, Token};

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
        let suffixes = self.suffixes.get(..count).unwrap_or(&self.suffixes);
        ChainHead { prefix: &self.prefix, suffixes }
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
    /// the tree nests no deeper than the parser would read it.
    pub fn splice_prefix(&mut self) {
        while matches!(self.prefix, Expression::Suffixed(_)) {
            if let Expression::Suffixed(inner) = self.prefix.take() {
                let Self { prefix, suffixes, .. } = *inner;
                self.prefix = prefix;
                self.suffixes.prepend(suffixes);
            }
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
    /// nests no deeper than the parser would read it. Each such operand's
    /// own operands are taken as they are.
    pub fn splice_operands(&mut self) {
        let level = self.precedence().0;
        let is_bare_run = |operand: &Expression| matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 == level);
        if !is_bare_run(&self.first) && !self.links.iter().any(|link| is_bare_run(&link.operand)) {
            return;
        }
        let placeholder = BinaryLink { op: BinOp::Or, operand: Expression::Error(Span::default()) };
        let old_links = mem::replace(&mut self.links, OneOrMore::new(placeholder));
        let mut links = Vec::with_capacity(old_links.len());
        let (first, first_links) = split_run(self.first.take(), level);
        self.first = first;
        links.extend(first_links.into_iter().flat_map(OneOrMore::into_vec));
        for BinaryLink { op, operand } in old_links.into_vec() {
            let (operand_first, operand_links) = split_run(operand, level);
            links.push(BinaryLink { op, operand: operand_first });
            links.extend(operand_links.into_iter().flat_map(OneOrMore::into_vec));
        }
        if let Some(spliced) = OneOrMore::from_vec(links) {
            self.links = spliced;
        }
    }
}

/// `operand`'s first operand and links when it's a run at `level`, and
/// `operand` itself otherwise.
fn split_run(operand: Expression, level: u8) -> (Expression, Option<OneOrMore<BinaryLink>>) {
    if let Expression::BinaryOp(inner) = operand {
        if inner.precedence().0 != level {
            return (Expression::BinaryOp(inner), None);
        }
        let BinaryOp { first, links, .. } = *inner;
        return (first, Some(links));
    }
    (operand, None)
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
}
