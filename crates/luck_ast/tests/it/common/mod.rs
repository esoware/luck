use luck_allocator::Allocator;
use luck_ast::{SpanCounter, Synth};

/// An arena and a span counter for a test's synthesizer.
#[derive(Debug, Default)]
pub(crate) struct Fixture {
    allocator: Allocator,
    spans: SpanCounter,
}

impl Fixture {
    /// A fixture whose spans start at `offset`, so its trees span apart from
    /// a default fixture's.
    pub(crate) fn starting_at(offset: u32) -> Self {
        Self { allocator: Allocator::new(), spans: SpanCounter::starting_at(offset) }
    }

    pub(crate) const fn synth(&self) -> Synth<'_> {
        Synth::new(&self.allocator, &self.spans)
    }
}
