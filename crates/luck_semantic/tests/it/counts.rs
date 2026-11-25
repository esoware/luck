//! The parse's counts size every table analysis fills, so a clean parse
//! counts at least as many of each as analysis makes, and one scope per
//! block exactly.

use luck_allocator::Allocator;
use luck_semantic::SemanticBuilder;

use crate::common::for_each_valid_source;

fn assert_counts_cover_the_tables(source: &str) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}\n{source}", parsed.errors.first());
    let counts = parsed.counts;
    let chunk = allocator.alloc(parsed.chunk);
    let semantic = SemanticBuilder::new().with_counts(counts).build(chunk).semantic;
    let tree = semantic.scoping();
    let declarations = counts.declarations as usize;
    let name_uses = counts.name_uses as usize;
    assert_eq!(tree.scopes().len(), counts.blocks as usize, "{source}");
    assert!(tree.symbols().len() <= declarations, "{counts:?}\n{source}");
    assert!(tree.references().len() <= name_uses, "{counts:?}\n{source}");
    assert!(tree.names().count() <= declarations + name_uses, "{counts:?}\n{source}");
}

#[test]
fn counts_cover_what_analysis_makes_of_every_valid_source() {
    for_each_valid_source(assert_counts_cover_the_tables);
}
