use rustc_hash::FxHashSet;

/// Indexes below this are claimed in one bit each.
const LOW_INDEXES: usize = 64;

/// Which indexes are claimed in each scope, with every scope's first
/// unclaimed index kept at hand.
///
/// A scope's first 64 indexes are bits of one word, which is all most scopes
/// ever claim, and the rest share one set, so memory follows the number of
/// scopes plus the number of claims, not scopes times indexes.
#[derive(Default)]
pub(super) struct ScopeClaims {
    low: Vec<u64>,
    high: FxHashSet<(usize, usize)>,
    first_free: Vec<usize>,
}

impl ScopeClaims {
    /// Forgets every claim, for scopes `0..scope_count`.
    pub(super) fn reset(&mut self, scope_count: usize) {
        self.low.clear();
        self.low.resize(scope_count, 0);
        self.high.clear();
        self.first_free.clear();
        self.first_free.resize(scope_count, 0);
    }

    /// Every index below this one is claimed in `scope`.
    pub(super) fn first_free(&self, scope: usize) -> usize {
        self.first_free.get(scope).copied().unwrap_or(0)
    }

    pub(super) fn claim(&mut self, scopes: &[usize], index: usize) {
        for &scope in scopes {
            self.claim_in(scope, index);
        }
    }

    fn claim_in(&mut self, scope: usize, index: usize) {
        if index < LOW_INDEXES {
            if let Some(word) = self.low.get_mut(scope) {
                *word |= 1u64 << index;
            }
        } else {
            self.high.insert((scope, index));
        }
        if self.first_free(scope) != index {
            return;
        }
        let mut first_free = index.saturating_add(1);
        if let Some(&word) = self.low.get(scope)
            && first_free < LOW_INDEXES
        {
            first_free = usize::try_from(word.trailing_ones()).unwrap_or(LOW_INDEXES);
        }
        while first_free >= LOW_INDEXES && self.high.contains(&(scope, first_free)) {
            first_free = first_free.saturating_add(1);
        }
        if let Some(slot) = self.first_free.get_mut(scope) {
            *slot = first_free;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_free_moves_past_every_claimed_index() {
        let mut claims = ScopeClaims::default();
        claims.reset(10);
        assert_eq!(claims.first_free(7), 0);
        claims.claim(&[7], 1);
        assert_eq!(claims.first_free(7), 0);
        claims.claim(&[7, 9], 0);
        assert_eq!(claims.first_free(7), 2);
        assert_eq!(claims.first_free(9), 1);
    }

    #[test]
    fn first_free_crosses_from_the_bits_into_the_set() {
        let mut claims = ScopeClaims::default();
        claims.reset(2);
        for index in (0..70).rev() {
            claims.claim(&[1], index);
        }
        assert_eq!(claims.first_free(1), 70);
        claims.claim(&[1], 71);
        claims.claim(&[1], 70);
        assert_eq!(claims.first_free(1), 72);
        assert_eq!(claims.first_free(0), 0);
    }
}
