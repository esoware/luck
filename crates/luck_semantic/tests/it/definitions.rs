use luck_allocator::Allocator;
use luck_ast::{AstType, GetSpan as _};
use luck_semantic::{ReferenceKind, SemanticBuilder};
use luck_testgen::definition_fixtures;

#[test]
fn a_declaration_binds_no_local_and_reads_only_what_its_types_do() {
    let allocator = Allocator::new();
    let source = "\
declare count: number
declare function first<T>(list: { T }, fallback: typeof(count)): T
declare extern type Part extends Instance with
    Name: string
    function Clone(self, depth: number): Part
end
local alias = count";
    let parsed = luck_parser::parse_definitions(&allocator, source.as_bytes());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    let built = SemanticBuilder::new()
        .with_counts(parsed.counts)
        .with_check(true)
        .with_nodes(true)
        .build(&parsed.chunk);
    assert!(built.errors.is_empty(), "{:?}", built.errors);
    let scoping = built.semantic.scoping();
    let symbols: Vec<_> =
        scoping.symbols().iter().map(|symbol| scoping.name(symbol.name)).collect();
    assert_eq!(symbols, ["alias"]);
    let references: Vec<_> = scoping
        .references()
        .iter()
        .map(|reference| {
            let name = scoping.name(reference.name);
            (name, reference.kind, reference.is_in_type, reference.is_global())
        })
        .collect();
    assert_eq!(
        references,
        [("count", ReferenceKind::Read, true, true), ("count", ReferenceKind::Read, false, true)]
    );
    let kinds: Vec<_> = built.semantic.nodes().iter().map(|node| node.kind().ty()).collect();
    for declaration in
        [AstType::DeclaredGlobal, AstType::DeclaredFunction, AstType::DeclaredExternType]
    {
        assert!(kinds.contains(&declaration), "no {declaration:?} node");
    }
    let methods = kinds.iter().filter(|kind| **kind == AstType::DeclaredFunction).count();
    assert_eq!(methods, 2, "the global function and the method");
}

/// Each fixture checks cleanly, and its nodes come in source preorder, each
/// inside its parent.
#[test]
fn every_definition_fixture_checks_cleanly_with_its_nodes_in_source_order() {
    for fixture in definition_fixtures() {
        let allocator = Allocator::new();
        let parsed = luck_parser::parse_definitions(&allocator, fixture.source.as_bytes());
        let name = fixture.relative_path.display();
        assert!(parsed.errors.is_empty(), "{name}: {:?}", parsed.errors);
        let built = SemanticBuilder::new().with_check(true).with_nodes(true).build(&parsed.chunk);
        assert!(built.errors.is_empty(), "{name}: {:?}", built.errors);
        let nodes = built.semantic.nodes();
        let mut previous_start = 0;
        for node in nodes.iter().skip(1) {
            let (id, span) = (node.id(), node.span());
            assert!(
                previous_start <= span.start,
                "{name}: {id:?} starts before the node before it"
            );
            previous_start = span.start;
            let parent = nodes.parent_id(id).expect("every node but the chunk has a parent");
            let around = nodes.get(parent).span();
            assert!(
                around.start <= span.start && span.end <= around.end,
                "{name}: {id:?} in {parent:?}"
            );
        }
    }
}
