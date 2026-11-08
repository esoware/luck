/// Scope-id set as a bitset: scope ids are small and dense, and the hot
/// operations are whole-set union and intersection.
#[derive(Default)]
pub(super) struct ScopeSet(Vec<u64>);

impl ScopeSet {
    const WORD_SHIFT: u32 = 6;
    const BIT_MASK: usize = 63;

    const fn bit(scope: usize) -> u64 {
        1u64 << (scope & Self::BIT_MASK)
    }

    pub(super) fn insert(&mut self, scope: usize) {
        let word_index = scope >> Self::WORD_SHIFT;
        if word_index >= self.0.len() {
            self.0.resize(word_index.saturating_add(1), 0);
        }
        if let Some(word) = self.0.get_mut(word_index) {
            *word |= Self::bit(scope);
        }
    }

    pub(super) fn contains(&self, scope: usize) -> bool {
        self.0.get(scope >> Self::WORD_SHIFT).is_some_and(|word| word & Self::bit(scope) != 0)
    }

    pub(super) fn union_with(&mut self, other: &Self) {
        if other.0.len() > self.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (destination, source) in self.0.iter_mut().zip(&other.0) {
            *destination |= source;
        }
    }

    pub(super) fn intersects(&self, other: &Self) -> bool {
        self.0.iter().zip(&other.0).any(|(left, right)| left & right != 0)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word_index, &word)| {
            (0..64)
                .filter(move |bit| word & (1 << bit) != 0)
                .map(move |bit| (word_index << Self::WORD_SHIFT) | bit)
        })
    }
}
