use luck_ast::AstType;
use luck_semantic::AstTypesBitset;

#[test]
fn holds_and_lists_the_types_set() {
    let bitset = AstTypesBitset::from_types(&[AstType::Block, AstType::SuffixedExpression]);
    assert!(bitset.has(AstType::Block));
    assert!(!bitset.has(AstType::Chunk));
    let indices: Vec<usize> = bitset.indices().collect();
    assert_eq!(indices.len(), 2);
    assert!(indices.is_sorted());
    let mut only_chunk = AstTypesBitset::new();
    only_chunk.set(AstType::Chunk);
    assert!(!bitset.intersects(only_chunk));
    only_chunk.set(AstType::Block);
    assert!(bitset.intersects(only_chunk));
}
