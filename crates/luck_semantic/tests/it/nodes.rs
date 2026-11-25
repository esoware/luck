use luck_allocator::Allocator;
use luck_ast::{AstKind, AstType, GetSpan as _};
use luck_semantic::SemanticBuilder;

use crate::common::{NAMES_OF_EVERY_KIND, parse, semantic_for};

#[test]
fn node_at_finds_the_innermost_node_an_offset_touches() {
    let allocator = Allocator::new();
    let source = "local ab = f( ab )g()";
    let semantic = semantic_for(&allocator, source);
    let nodes = semantic.nodes();
    let node_at = |offset| {
        nodes.node_at(offset).map(|id| {
            let node = nodes.get(id);
            (node.kind().ty(), node.span().start, node.span().end)
        })
    };
    // `ab` is 6..8, the call `f( ab )` 11..18, and `g()` 18..21.
    assert_eq!(node_at(5), Some((AstType::LocalAssignment, 0, 18)));
    assert_eq!(node_at(6), Some((AstType::Name, 6, 8)));
    assert_eq!(node_at(8), Some((AstType::Name, 6, 8)), "a cursor just past a name");
    assert_eq!(node_at(13), Some((AstType::SuffixedExpression, 11, 18)));
    assert_eq!(node_at(14), Some((AstType::Name, 14, 16)));
    assert_eq!(node_at(18), Some((AstType::Name, 18, 19)), "the later of two that meet");
    assert_eq!(node_at(20), Some((AstType::SuffixedExpression, 18, 21)));
    assert_eq!(node_at(22), None);
}

#[test]
fn nodes_are_in_source_preorder_inside_their_parents() {
    let allocator = Allocator::new();
    let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND);
    let nodes = semantic.nodes();
    let chunk = nodes.iter().next().expect("the chunk has a node");
    assert_eq!(chunk.id().index(), 0);
    assert!(matches!(chunk.kind(), AstKind::Block(_)));
    assert_eq!(nodes.parent_id(chunk.id()), None);
    let mut previous_start = 0;
    for node in nodes.iter().skip(1) {
        let (id, span) = (node.id(), node.span());
        assert!(previous_start <= span.start, "{id:?} starts before the node before it");
        previous_start = span.start;
        let parent = nodes.parent_id(id).expect("every node but the chunk has a parent");
        assert!(parent < id, "{id:?} comes after its parent");
        let around = nodes.get(parent).span();
        assert!(around.start <= span.start && span.end <= around.end, "{id:?} in {parent:?}");
    }
    let unbuilt = SemanticBuilder::new().build(parse(&allocator, NAMES_OF_EVERY_KIND)).semantic;
    assert!(unbuilt.nodes().is_empty());
    let built = semantic.scoping().references().iter().map(|reference| reference.node);
    let unbuilt_nodes = unbuilt.scoping().references().iter().map(|reference| reference.node);
    assert!(built.eq(unbuilt_nodes), "node ids don't depend on building the table");
}

#[test]
fn tables_point_at_the_nodes_that_declare_and_open() {
    let allocator = Allocator::new();
    let semantic = semantic_for(
        &allocator,
        "\
const a = 1
local function f(b, ...) end
function t:m() end
for i = 1, 2 do end
for k in pairs(t) do end
do end",
    );
    let (scoping, nodes) = (semantic.scoping(), semantic.nodes());
    let declarations: Vec<(&str, AstType)> = scoping
        .symbols()
        .iter()
        .map(|symbol| (scoping.name(symbol.name), nodes.kind(symbol.declaration).ty()))
        .collect();
    assert_eq!(
        declarations,
        [
            ("a", AstType::TypedName),
            ("f", AstType::LocalFunction),
            ("b", AstType::TypedName),
            ("self", AstType::FunctionBody),
            ("i", AstType::TypedName),
            ("k", AstType::TypedName),
        ]
    );
    let read_only: Vec<&str> = scoping
        .symbols()
        .iter()
        .filter(|symbol| symbol.is_read_only)
        .map(|symbol| scoping.name(symbol.name))
        .collect();
    assert_eq!(read_only, ["a"]);
    let openers: Vec<AstType> =
        scoping.scopes().iter().map(|scope| nodes.kind(scope.node).ty()).collect();
    assert_eq!(
        openers,
        [
            AstType::Block,
            AstType::FunctionBody,
            AstType::FunctionBody,
            AstType::NumericFor,
            AstType::GenericFor,
            AstType::Block,
        ]
    );
    let body = scoping.scopes()[1].node;
    assert_eq!(nodes.get(body).scope().index(), 0, "a node is in the scope around it");
    let body_block = nodes.iter().find(|node| {
        nodes.parent_id(node.id()) == Some(body) && matches!(node.kind(), AstKind::Block(_))
    });
    assert_eq!(body_block.map(|block| block.scope().index()), Some(1));
    assert_eq!(
        nodes.ancestor_kinds(body).map(AstKind::ty).collect::<Vec<_>>(),
        [AstType::LocalFunction, AstType::Block]
    );
}
