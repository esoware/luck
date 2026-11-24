use std::iter;

use luck_ast::{AST_TYPE_MAX, AstType};

const WORD_COUNT: usize = (AST_TYPE_MAX as usize >> 6) + 1;

/// A set of [`AstType`]s, one bit each: the kinds of node a lint rule runs
/// on, or the kinds a tree holds, which
/// [`AstNodes::contains_any`](crate::AstNodes::contains_any) checks against
/// them. Ported from `oxc_semantic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AstTypesBitset([u64; WORD_COUNT]);

impl AstTypesBitset {
    pub const fn new() -> Self {
        Self([0; WORD_COUNT])
    }

    pub const fn from_types(types: &[AstType]) -> Self {
        let mut bitset = Self::new();
        let mut remaining = types;
        while let [first, rest @ ..] = remaining {
            bitset.set(*first);
            remaining = rest;
        }
        bitset
    }

    pub const fn has(self, ty: AstType) -> bool {
        let (word, mask) = word_and_mask(ty);
        self.0[word] & mask != 0
    }

    pub const fn set(&mut self, ty: AstType) {
        let (word, mask) = word_and_mask(ty);
        self.0[word] |= mask;
    }

    /// Whether a type is in both.
    pub fn intersects(self, other: Self) -> bool {
        self.0.iter().zip(other.0).any(|(&own, theirs)| own & theirs != 0)
    }

    /// The discriminant of each type in the set, in ascending order.
    pub fn indices(self) -> impl Iterator<Item = usize> {
        self.0.into_iter().enumerate().flat_map(|(word_index, word)| {
            let mut remaining = word;
            iter::from_fn(move || {
                let bit = (remaining != 0).then(|| remaining.trailing_zeros())?;
                remaining &= remaining - 1;
                Some(word_index * 64 + bit as usize)
            })
        })
    }
}

/// The word that holds `ty`'s bit, and the bit.
const fn word_and_mask(ty: AstType) -> (usize, u64) {
    let index = ty as usize;
    (index >> 6, 1 << (index & 63))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
