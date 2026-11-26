use std::borrow::Cow;

use luck_allocator::Allocator;
use luck_ast::Block;
use luck_type_checker::{Config, Host};

/// A host with no modules, for a checker that only loads definitions.
pub(crate) struct NoModules;

impl Host for NoModules {
    fn source(&self, _: &str) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn resolve(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    fn config(&self, _: &str, _: &mut Config<'_>) {}
}

/// `source`'s tree, which must parse cleanly, kept in `allocator` so it
/// outlives the analysis that borrows it.
pub(crate) fn parse<'ast>(allocator: &'ast Allocator, source: &'ast str) -> &'ast Block<'ast> {
    let parsed = luck_parser::parse(allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{source}: {:?}", parsed.errors);
    allocator.alloc(parsed.chunk)
}
