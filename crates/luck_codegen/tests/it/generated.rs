use luck_allocator::Allocator;
use luck_testgen::{generate_full_grammar, generate_runtime_safe};

use crate::common::assert_reprints;

/// The full-grammar generator reaches what fixtures rarely hold: every type
/// form, varargs, interpolation, and every operator side by side.
#[test]
fn generated_programs_compact_to_output_that_reads_back_as_the_same_tree() {
    for seed in 0..200 {
        for source in [generate_full_grammar(seed, 40), generate_runtime_safe(seed, 40)] {
            let allocator = Allocator::new();
            let parsed = luck_parser::parse(&allocator, source.as_bytes());
            assert!(parsed.errors.is_empty(), "seed {seed}: {:?}", parsed.errors);
            assert_reprints(&parsed.chunk);
        }
    }
}
