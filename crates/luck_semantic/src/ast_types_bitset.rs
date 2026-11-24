use core::iter;

use luck_ast::{AST_TYPE_MAX, AstType};
use luck_token::offset_to_index;

#[expect(clippy::as_conversions, reason = "a const has no other way to widen a `u8`")]
const WORD_COUNT: usize = (AST_TYPE_MAX as usize >> 6).saturating_add(1);

/// A set of [`AstType`]s, one bit each: the kinds of node a lint rule runs
/// on, or the kinds a tree holds, which
/// [`AstNodes::contains_any`](crate::AstNodes::contains_any) checks against
/// them. Ported from `oxc_semantic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AstTypesBitset([u64; WORD_COUNT]);

impl AstTypesBitset {
    #[must_use]
    pub const fn new() -> Self {
        Self([0; WORD_COUNT])
    }

    #[must_use]
    pub const fn from_types(types: &[AstType]) -> Self {
        let mut bitset = Self::new();
        let mut remaining = types;
        while let [first, rest @ ..] = remaining {
            bitset.set(*first);
            remaining = rest;
        }
        bitset
    }

    #[must_use]
    #[expect(clippy::indexing_slicing, reason = "the words hold a bit for every discriminant")]
    pub const fn has(self, ty: AstType) -> bool {
        let (word, mask) = word_and_mask(ty);
        self.0[word] & mask != 0
    }

    #[expect(clippy::indexing_slicing, reason = "the words hold a bit for every discriminant")]
    pub const fn set(&mut self, ty: AstType) {
        let (word, mask) = word_and_mask(ty);
        self.0[word] |= mask;
    }

    /// Whether a type is in both.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.0.iter().zip(other.0).any(|(&own, theirs)| own & theirs != 0)
    }

    /// The discriminant of each type in the set, in ascending order.
    pub fn indices(self) -> impl Iterator<Item = usize> {
        self.0.into_iter().zip(0usize..).flat_map(|(word, word_index)| {
            let mut remaining = word;
            iter::from_fn(move || {
                let bit = (remaining != 0).then(|| remaining.trailing_zeros())?;
                remaining &= remaining.wrapping_sub(1);
                Some(word_index.saturating_mul(64).saturating_add(offset_to_index(bit)))
            })
        })
    }
}

/// The word that holds `ty`'s bit, and the bit.
#[expect(clippy::as_conversions, reason = "a const fn has no other way to read a discriminant")]
const fn word_and_mask(ty: AstType) -> (usize, u64) {
    let index = ty as usize;
    (index >> 6, 1u64 << (index & 63))
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
