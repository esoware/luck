use core::iter::once;
use core::{fmt, mem};

use luck_allocator::{Allocator, Box, ListIntoIter};
use luck_token::{Associativity, BinOp, OperandSide, Span};

use crate::expr::{BinaryLink, BinaryOp, Expression, FunctionArgs, Suffix, SuffixedExpression};
use crate::leaf::Name;
use crate::types::TypeArgs;

/// A chain's prefix and its first few suffixes, read as one expression: what
/// a call calls, a method's receiver, or what a field or index reads from.
#[derive(Debug, Clone, Copy)]
pub struct ChainHead<'ast> {
    pub prefix: &'ast Expression<'ast>,
    pub suffixes: &'ast [Suffix<'ast>],
}

impl<'ast> ChainHead<'ast> {
    /// The prefix, when no suffix follows it.
    #[must_use]
    pub const fn as_expr(self) -> Option<&'ast Expression<'ast>> {
        if self.suffixes.is_empty() { Some(self.prefix) } else { None }
    }

    /// The name, when the head is nothing else.
    #[must_use]
    pub fn as_name(self) -> Option<&'ast Name<'ast>> {
        if let Some(Expression::Name(name)) = self.as_expr() { Some(name) } else { None }
    }

    /// `None` for a prefix other than a name, or a suffix other than a
    /// field.
    #[must_use]
    pub fn dotted_path(self) -> Option<DottedPath<'ast>> {
        let Expression::Name(root) = self.prefix else {
            return None;
        };
        let is_dotted = self.suffixes.iter().all(|suffix| matches!(suffix, Suffix::Field(_)));
        is_dotted.then_some(DottedPath { root, fields: self.suffixes })
    }

    #[must_use]
    pub fn split_last(self) -> Option<(Self, &'ast Suffix<'ast>)> {
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
impl<'ast> From<&'ast Expression<'ast>> for ChainHead<'ast> {
    fn from(expr: &'ast Expression<'ast>) -> Self {
        if let Expression::Suffixed(chain) = expr {
            Self { prefix: &chain.prefix, suffixes: &chain.suffixes }
        } else {
            Self { prefix: expr, suffixes: &[] }
        }
    }
}

/// A `Name(.Name)*` chain, which displays as `a.b.c`.
#[derive(Debug, Clone, Copy)]
pub struct DottedPath<'ast> {
    root: &'ast Name<'ast>,
    /// Every one a [`Suffix::Field`].
    fields: &'ast [Suffix<'ast>],
}

impl<'ast> DottedPath<'ast> {
    #[must_use]
    pub const fn root(self) -> &'ast Name<'ast> {
        self.root
    }

    /// The names' texts, root first.
    pub fn segments(self) -> impl Iterator<Item = &'ast str> + Clone {
        let fields = self.fields.iter().filter_map(|suffix| {
            if let Suffix::Field(name) = suffix { Some(name.text) } else { None }
        });
        once(self.root.text).chain(fields)
    }

    /// How many names it has, the root's included.
    #[must_use]
    pub const fn segment_count(self) -> usize {
        self.fields.len().saturating_add(1)
    }
}

impl fmt::Display for DottedPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut separator = "";
        for segment in self.segments() {
            f.write_str(separator)?;
            f.write_str(segment)?;
            separator = ".";
        }
        Ok(())
    }
}

/// A call in a [`SuffixedExpression`], and the part of the chain before it.
#[derive(Debug, Clone, Copy)]
pub struct Call<'ast> {
    /// The receiver, for a method call.
    pub callee: ChainHead<'ast>,
    pub method: Option<&'ast Name<'ast>>,
    /// See [`MethodCall::type_args`](crate::MethodCall::type_args).
    pub type_args: Option<&'ast TypeArgs<'ast>>,
    pub args: &'ast FunctionArgs<'ast>,
}

impl<'ast> Call<'ast> {
    /// `None` for a suffix other than a call.
    #[must_use]
    pub fn new(callee: ChainHead<'ast>, suffix: &'ast Suffix<'ast>) -> Option<Self> {
        match suffix {
            Suffix::Call(args) => Some(Self { callee, method: None, type_args: None, args }),
            Suffix::MethodCall(call) => Some(Self {
                callee,
                method: Some(&call.name),
                type_args: call.type_args.as_deref(),
                args: &call.args,
            }),
            Suffix::Field(_) | Suffix::Index { .. } | Suffix::TypeInstantiation { .. } => None,
        }
    }

    #[must_use]
    pub fn span(self) -> Span {
        self.callee.span().merge(self.args.span())
    }
}

impl<'ast> SuffixedExpression<'ast> {
    /// The prefix and the first `count` suffixes, or all of them when there
    /// are fewer.
    #[must_use]
    pub fn head(&self, count: usize) -> ChainHead<'_> {
        let suffixes: &[Suffix<'_>] = &self.suffixes;
        ChainHead { prefix: &self.prefix, suffixes: suffixes.get(..count).unwrap_or(suffixes) }
    }

    /// Every call in the chain, first to last.
    pub fn calls(&self) -> impl Iterator<Item = Call<'_>> {
        self.suffixes.iter().enumerate().filter_map(|(i, suffix)| Call::new(self.head(i), suffix))
    }

    /// The call the chain ends in, if it ends in one.
    #[must_use]
    pub fn last_call(&self) -> Option<Call<'_>> {
        let (last, before) = self.suffixes.split_last()?;
        Call::new(ChainHead { prefix: &self.prefix, suffixes: before }, last)
    }

    /// Whether the chain ends in a call, which makes it one too.
    #[must_use]
    pub fn is_call(&self) -> bool {
        matches!(self.suffixes.last(), Suffix::Call(_) | Suffix::MethodCall(_))
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
    pub fn splice_prefix(&mut self, allocator: &'ast Allocator) {
        if !matches!(self.prefix, Expression::Suffixed(_)) {
            return;
        }
        // Each chain's suffixes swap in for the ones after them, which wait
        // here, outermost first.
        let mut later = Vec::new();
        let mut prefix = self.prefix.take();
        while let Expression::Suffixed(inner) = prefix {
            let Self { prefix: inner_prefix, suffixes, .. } = Box::unbox(inner);
            later.push(mem::replace(&mut self.suffixes, suffixes));
            prefix = inner_prefix;
        }
        self.prefix = prefix;
        for suffixes in later.into_iter().rev() {
            for suffix in suffixes {
                self.suffixes.push(suffix, allocator);
            }
        }
    }
}

impl<'ast> BinaryOp<'ast> {
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
    pub fn splice_operands(&mut self, allocator: &'ast Allocator) {
        let level = self.precedence().0;
        let is_bare_run = |operand: &Expression<'_>| matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 == level);
        if !is_bare_run(&self.first) && !self.links.iter().any(|link| is_bare_run(&link.operand)) {
            return;
        }
        // The links still to place, the innermost run's on top. A bare first
        // operand's links swap in for the ones after them, which wait here.
        let mut pending = Vec::new();
        let mut first = self.first.take();
        while let Expression::BinaryOp(inner) = first {
            if inner.precedence().0 != level {
                first = Expression::BinaryOp(inner);
                break;
            }
            let Self { first: inner_first, links, .. } = Box::unbox(inner);
            pending.push(mem::replace(&mut self.links, links).into_iter());
            first = inner_first;
        }
        self.first = first;
        pending.push(self.links.split_off_rest().into_iter());
        // The one link left.
        let head = self.links.last_mut();
        head.operand = leftmost_operand(head.operand.take(), level, &mut pending);
        while let Some(links) = pending.last_mut() {
            let Some(link) = links.next() else {
                pending.pop();
                continue;
            };
            let operand = leftmost_operand(link.operand, level, &mut pending);
            self.links.push(BinaryLink { op: link.op, operand }, allocator);
        }
    }
}

/// `operand` out of the bare runs at `level` it opens, whose links go on top
/// of `pending`, to follow it.
fn leftmost_operand<'ast>(
    mut operand: Expression<'ast>,
    level: u8,
    pending: &mut Vec<ListIntoIter<'ast, BinaryLink<'ast>>>,
) -> Expression<'ast> {
    loop {
        let Expression::BinaryOp(inner) = operand else {
            return operand;
        };
        if inner.precedence().0 != level {
            return Expression::BinaryOp(inner);
        }
        let BinaryOp { first, links, .. } = Box::unbox(inner);
        pending.push(links.into_iter());
        operand = first;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Fixture;
    use crate::one_or_more::OneOrMore;

    fn unspliced_chain<'ast>(
        prefix: Expression<'ast>,
        suffix: Suffix<'ast>,
        allocator: &'ast Allocator,
    ) -> Expression<'ast> {
        let suffixes = OneOrMore::new(suffix, allocator);
        let chain = SuffixedExpression { span: Span::default(), prefix, suffixes };
        Expression::Suffixed(Box::new_in(chain, allocator))
    }

    fn unspliced_run<'ast>(
        first: Expression<'ast>,
        op: BinOp,
        operand: Expression<'ast>,
        allocator: &'ast Allocator,
    ) -> Expression<'ast> {
        let links = OneOrMore::new(BinaryLink { op, operand }, allocator);
        let run = BinaryOp { span: Span::default(), first, links };
        Expression::BinaryOp(Box::new_in(run, allocator))
    }

    #[test]
    fn dotted_path_reads_a_name_and_its_fields_only() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let path = synth.field(synth.field(synth.name_expr("a"), "b"), "c");
        let Expression::Suffixed(dotted_chain) = &path else {
            panic!("expected a suffixed expression");
        };
        let dotted = dotted_chain.head(2).dotted_path().expect("a.b.c is dotted");
        assert_eq!(dotted.root().text, "a");
        assert_eq!(dotted.segments().collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(dotted.segment_count(), 3);
        assert_eq!(dotted.to_string(), "a.b.c");

        let called = synth.field(synth.call(synth.name_expr("f"), []), "x");
        let Expression::Suffixed(called_chain) = &called else {
            panic!("expected a suffixed expression");
        };
        assert!(called_chain.head(2).dotted_path().is_none());
    }

    #[test]
    fn splice_prefix_joins_every_suffixed_prefix_in_order() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let called = synth.call(synth.name_expr("f"), []);
        let field = unspliced_chain(called, Suffix::Field(synth.name("a")), allocator);
        let Expression::Suffixed(mut chain) =
            unspliced_chain(field, Suffix::Field(synth.name("b")), allocator)
        else {
            panic!("expected a suffixed expression");
        };
        chain.splice_prefix(allocator);
        assert!(matches!(chain.prefix, Expression::Name(_)));
        assert!(matches!(*chain.suffixes, [Suffix::Call(_), Suffix::Field(_), Suffix::Field(_)]));
    }

    #[test]
    fn splice_operands_joins_bare_runs_at_its_level_in_order() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let name = |name: &str| synth.name_expr(name);
        // (a + b) - (c * d + e), with no parentheses: a + b - c * d + e.
        let left = unspliced_run(name("a"), BinOp::Add, name("b"), allocator);
        let product = unspliced_run(name("c"), BinOp::Mul, name("d"), allocator);
        let right = unspliced_run(product, BinOp::Add, name("e"), allocator);
        let Expression::BinaryOp(mut run) = unspliced_run(left, BinOp::Sub, right, allocator)
        else {
            panic!("expected a binary run");
        };
        run.splice_operands(allocator);
        let ops: Vec<BinOp> = run.links.iter().map(|link| link.op).collect();
        assert_eq!(ops, [BinOp::Add, BinOp::Sub, BinOp::Add]);
        assert!(matches!(run.first, Expression::Name(_)));
        assert!(matches!(run.links[1].operand, Expression::BinaryOp(_)), "`c * d` stays a run");
    }

    #[test]
    fn splicing_joins_runs_nested_any_number_of_levels_deep() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let name = || synth.name_expr("x");
        let mut sum = name();
        let mut concat = name();
        let mut path = name();
        for _ in 0..50_000u32 {
            sum = unspliced_run(sum, BinOp::Add, name(), allocator);
            concat = unspliced_run(name(), BinOp::Concat, concat, allocator);
            path = unspliced_chain(path, Suffix::Field(synth.name("a")), allocator);
        }
        for nested in [sum, concat] {
            let Expression::BinaryOp(mut run) = nested else {
                panic!("expected a binary run");
            };
            run.splice_operands(allocator);
            assert_eq!(run.links.len(), 50_000);
            assert!(matches!(run.first, Expression::Name(_)));
            assert!(run.links.iter().all(|link| matches!(link.operand, Expression::Name(_))));
        }
        let Expression::Suffixed(mut chain) = path else {
            panic!("expected a suffixed expression");
        };
        chain.splice_prefix(allocator);
        assert_eq!(chain.suffixes.len(), 50_000);
        assert!(matches!(chain.prefix, Expression::Name(_)));
    }
}
