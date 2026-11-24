use luck_allocator::Allocator;
use luck_testgen::{generate_full_grammar, generate_runtime_safe};
use luck_token::LuaVersion;

use crate::common::assert_reprints;

/// The full-grammar generator reaches what fixtures rarely hold: goto, every
/// Luau type form, varargs, interpolation, and every operator side by side.
#[test]
fn generated_programs_compact_to_output_that_compacts_to_itself() {
    for version in LuaVersion::ALL {
        for seed in 0..200 {
            for source in
                [generate_full_grammar(seed, version, 40), generate_runtime_safe(seed, version, 40)]
            {
                let allocator = Allocator::new();
                let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
                assert!(parsed.errors.is_empty(), "{version:?} seed {seed}: {:?}", parsed.errors);
                assert_reprints(&parsed.chunk);
            }
        }
    }
}
