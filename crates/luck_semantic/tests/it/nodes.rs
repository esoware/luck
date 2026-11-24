use luck_allocator::Allocator;
use luck_ast::{AstKind, AstType, GetSpan as _};
use luck_semantic::SemanticBuilder;
use luck_token::LuaVersion;

use crate::common::{NAMES_OF_EVERY_KIND, parse, semantic_for};

#[test]
fn node_at_finds_the_innermost_node_an_offset_touches() {
    let allocator = Allocator::new();
    let source = "local <const> ab = f( ab )g()";
    let semantic = semantic_for(&allocator, source, LuaVersion::Lua55);
    let nodes = semantic.nodes();
    let node_at = |offset| {
        nodes.node_at(offset).map(|id| {
            let node = nodes.get(id);
            (node.kind().ty(), node.span().start, node.span().end)
        })
    };
    // `<const>` is 6..13, `ab` 14..16, the call `f( ab )` 19..26, and
    // `g()` 26..29.
    assert_eq!(node_at(5), Some((AstType::LocalAssignment, 0, 26)));
    assert_eq!(node_at(6), Some((AstType::Attribute, 6, 13)));
    assert_eq!(node_at(8), Some((AstType::Name, 7, 12)));
    assert_eq!(node_at(14), Some((AstType::Name, 14, 16)));
    assert_eq!(node_at(16), Some((AstType::Name, 14, 16)), "a cursor just past a name");
    assert_eq!(node_at(21), Some((AstType::SuffixedExpression, 19, 26)));
    assert_eq!(node_at(22), Some((AstType::Name, 22, 24)));
    assert_eq!(node_at(26), Some((AstType::Name, 26, 27)), "the later of two that meet");
    assert_eq!(node_at(28), Some((AstType::SuffixedExpression, 26, 29)));
    assert_eq!(node_at(30), None);
}

#[test]
fn nodes_are_in_source_preorder_inside_their_parents() {
    let allocator = Allocator::new();
    let semantic = semantic_for(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54);
    let nodes = semantic.nodes();
    let chunk = nodes.iter().next().expect("the chunk has a node");
    assert_eq!(chunk.id().index(), 0);
    assert!(matches!(chunk.kind(), AstKind::Chunk(_)));
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
    let unbuilt = SemanticBuilder::new()
        .build(parse(&allocator, NAMES_OF_EVERY_KIND, LuaVersion::Lua54))
        .semantic;
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
local a <const> = 1
local function f(b, ...c) end
function t:m() end
for i = 1, 2 do end
for k in pairs(t) do end
do end",
        LuaVersion::Lua55,
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
            ("a", AstType::AttributedName),
            ("f", AstType::LocalFunction),
            ("b", AstType::Parameter),
            ("c", AstType::VarArgParam),
            ("self", AstType::FunctionBody),
            ("i", AstType::NumericFor),
            ("k", AstType::Parameter),
        ]
    );
    let read_only: Vec<&str> = scoping
        .symbols()
        .iter()
        .filter(|symbol| symbol.is_read_only)
        .map(|symbol| scoping.name(symbol.name))
        .collect();
    assert_eq!(read_only, ["a", "c", "i", "k"]);
    let openers: Vec<AstType> =
        scoping.scopes().iter().map(|scope| nodes.kind(scope.node).ty()).collect();
    assert_eq!(
        openers,
        [
            AstType::Chunk,
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
        [AstType::LocalFunction, AstType::Block, AstType::Chunk,]
    );
}
