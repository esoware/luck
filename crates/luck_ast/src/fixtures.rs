use luck_allocator::Allocator;
use luck_token::{BinOp, Span};

use crate::expr::{BinaryLink, Expression, Suffix};
use crate::one_or_more::OneOrMore;
use crate::synth::{SpanCounter, Synth};

/// An arena and a span counter for a test's synthesizer.
#[derive(Debug, Default)]
pub(crate) struct Fixture {
    allocator: Allocator,
    spans: SpanCounter,
}

impl Fixture {
    pub(crate) const fn allocator(&self) -> &Allocator {
        &self.allocator
    }

    pub(crate) const fn synth(&self) -> Synth<'_> {
        Synth::new(&self.allocator, &self.spans)
    }

    /// `first op operand` as a run of its own, which [`Synth::binary_op`]
    /// would join an operand at its level into.
    pub(crate) fn unspliced_run<'ast>(
        &'ast self,
        first: Expression<'ast>,
        op: BinOp,
        operand: Expression<'ast>,
    ) -> Expression<'ast> {
        let links = OneOrMore::new(BinaryLink::new(op, operand), &self.allocator);
        Expression::new_binary_op(Span::default(), first, links, &self.allocator)
    }

    /// `prefix` and `suffix` as a chain of their own, which the synthesizer
    /// would join a suffixed prefix into.
    pub(crate) fn unspliced_chain<'ast>(
        &'ast self,
        prefix: Expression<'ast>,
        suffix: Suffix<'ast>,
    ) -> Expression<'ast> {
        let suffixes = OneOrMore::new(suffix, &self.allocator);
        Expression::new_suffixed(Span::default(), prefix, suffixes, &self.allocator)
    }
}
