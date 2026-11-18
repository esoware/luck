use luck_allocator::Allocator;

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
}
