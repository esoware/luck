//! What the tables of every valid source hold.
//!
//! The node table is in source preorder, so it's sorted by where nodes
//! start, and each reference's and declaration's name finds its row there,
//! and scopes, symbols, and references point into it.

use luck_allocator::Allocator;
use luck_ast::{AstKind, Expression, GetSpan as _};
use luck_semantic::SemanticBuilder;
use luck_token::LuaVersion;

use crate::common::for_each_valid_source;

fn assert_tables_agree(source: &str, version: LuaVersion) {
    let allocator = Allocator::new();
    let parsed = luck_parser::parse(&allocator, source.as_bytes(), version);
    let chunk = allocator.alloc(parsed.chunk);
    let built = SemanticBuilder::new()
        .with_counts(parsed.counts)
        .with_nodes(true)
        .with_requires(true)
        .build(chunk);
    let semantic = built.semantic;
    let (tree, nodes) = (semantic.scoping(), semantic.nodes());
    let mut previous_start = 0;
    for (id, node) in nodes.iter() {
        let span = node.span();
        assert!(previous_start <= span.start, "{id:?} starts before the node before it\n{source}");
        previous_start = span.start;
        let Some(parent) = nodes.parent_id(id) else {
            assert_eq!(id.index(), 0, "only the chunk has no parent");
            continue;
        };
        assert!(parent < id, "{id:?} comes after its parent\n{source}");
        let around = nodes.get(parent).span();
        assert!(around.start <= span.start, "{id:?} starts inside {parent:?}\n{source}");
        assert!(span.end <= around.end, "{id:?} ends inside {parent:?}\n{source}");
        if let AstKind::Name(name) = node.kind()
            && let Some(reference) = tree.reference_of(name)
        {
            assert_eq!(tree.reference(reference).node, id, "{name:?}\n{source}");
        }
    }
    for reference in tree.references() {
        let AstKind::Name(name) = nodes.kind(reference.node) else {
            panic!("{reference:?} points at no name\n{source}");
        };
        assert_eq!(name.span, reference.span, "{source}");
    }
    for symbol in tree.symbols() {
        assert!(symbol.declaration.index() < nodes.len(), "{symbol:?}\n{source}");
        let declared = nodes.get(symbol.declaration).span();
        let span = symbol.definition_span;
        assert!(declared.start <= span.start && span.end <= declared.end, "{symbol:?}\n{source}");
    }
    for scope in tree.scopes() {
        assert!(scope.node.index() < nodes.len(), "{scope:?}\n{source}");
    }
    for site in semantic.requires() {
        let Expression::Name(root) = site.call.callee.prefix else {
            panic!("a require site's callee starts with a name\n{source}");
        };
        assert!(tree.reaches_host_global(root), "{source}");
    }
}

#[test]
fn every_valid_source_s_tables_agree() {
    for_each_valid_source(assert_tables_agree);
}
