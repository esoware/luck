use rustc_hash::FxHashSet;

/// Which indexes are claimed in each scope, with every scope's first
/// unclaimed index kept at hand.
///
/// One table holds every claim, so memory follows the number of scopes plus
/// the number of claims, not scopes times indexes, and finding a free index
/// doesn't rescan the claimed ones.
#[derive(Default)]
pub(super) struct ScopeClaims {
    claimed: FxHashSet<(usize, usize)>,
    first_free_by_scope: Vec<usize>,
}

impl ScopeClaims {
    /// Every index below this one is claimed in `scope`.
    pub(super) fn first_free(&self, scope: usize) -> usize {
        self.first_free_by_scope.get(scope).copied().unwrap_or(0)
    }

    pub(super) fn is_claimed(&self, scope: usize, index: usize) -> bool {
        self.claimed.contains(&(scope, index))
    }

    pub(super) fn claim(&mut self, scopes: &[usize], index: usize) {
        for &scope in scopes {
            self.claimed.insert((scope, index));
            if scope >= self.first_free_by_scope.len() {
                self.first_free_by_scope.resize(scope.saturating_add(1), 0);
            }
            if let Some(first_free) = self.first_free_by_scope.get_mut(scope) {
                while self.claimed.contains(&(scope, *first_free)) {
                    *first_free = first_free.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_free_moves_past_every_claimed_index() {
        let mut claims = ScopeClaims::default();
        assert_eq!(claims.first_free(7), 0);
        claims.claim(&[7], 1);
        assert_eq!(claims.first_free(7), 0);
        claims.claim(&[7, 9], 0);
        assert_eq!(claims.first_free(7), 2);
        assert_eq!(claims.first_free(9), 1);
        assert!(claims.is_claimed(9, 0));
        assert!(!claims.is_claimed(9, 1));
    }
}
