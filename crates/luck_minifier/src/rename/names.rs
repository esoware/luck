use luck_allocator::Allocator;
use luck_token::{LuaVersion, TokenKind};
use rustc_hash::FxHashSet;

const FIRST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyzABDEFGHIJMNOPQRSTVWXYZ_";
const REST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyz_ABDEFGHIJMNOPQRSTVWXYZ0123456789";

/// The first character comes from `FIRST_CHARS` (53 of them) and each later
/// one from `REST_CHARS` (63, digits included).
///
/// Indices 0 through 52 are one character, the next 53 * 63 are two, the next 53 * 63^2 are three,
/// and so on. Past the first 53, index `53 + n` is name `n / 63` followed by `REST_CHARS[n % 63]`.
///
/// The sequence includes keywords. Skipping keywords and names already in use
/// is the renamer's job.
fn push_name_for_index(index: usize, name: &mut String) {
    let Some(multi_char_index) = index.checked_sub(FIRST_CHARS.len()) else {
        name.extend(FIRST_CHARS.get(index).copied().map(char::from));
        return;
    };
    let rest_base = REST_CHARS.len();
    push_name_for_index(multi_char_index.div_euclid(rest_base), name);
    name.extend(REST_CHARS.get(multi_char_index.rem_euclid(rest_base)).copied().map(char::from));
}

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

    pub(super) fn is_claimed(&self, scope: usize, index: usize) -> bool {
        if index < LOW_INDEXES {
            self.low.get(scope).is_some_and(|&word| (word >> index) & 1 != 0)
        } else {
            self.high.contains(&(scope, index))
        }
    }

    pub(super) fn claim(&mut self, scopes: &[usize], index: usize) {
        for &scope in scopes {
            self.claim_in(scope, index);
        }
    }

    pub(super) fn claim_in(&mut self, scope: usize, index: usize) {
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

struct Candidate<'ast> {
    name: &'ast str,
    is_reserved: bool,
}

/// Each name and its keyword check, computed once and shared by every slot
/// of every round.
pub(super) struct CandidatePool<'ast> {
    allocator: &'ast Allocator,
    version: LuaVersion,
    candidates: Vec<Candidate<'ast>>,
    scratch: String,
}

impl<'ast> CandidatePool<'ast> {
    pub(super) const fn new(allocator: &'ast Allocator, version: LuaVersion) -> Self {
        Self { allocator, version, candidates: Vec::new(), scratch: String::new() }
    }

    /// The candidate at `index` and whether it's a keyword.
    pub(super) fn get(&mut self, index: usize) -> Option<(&'ast str, bool)> {
        while self.candidates.len() <= index {
            self.scratch.clear();
            push_name_for_index(self.candidates.len(), &mut self.scratch);
            let is_reserved = TokenKind::is_reserved(&self.scratch, self.version);
            let name = self.allocator.alloc_str(&self.scratch);
            self.candidates.push(Candidate { name, is_reserved });
        }
        self.candidates.get(index).map(|candidate| (candidate.name, candidate.is_reserved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_for_index(index: usize) -> String {
        let mut name = String::new();
        push_name_for_index(index, &mut name);
        name
    }

    #[test]
    fn single_chars_come_first() {
        assert_eq!(name_for_index(0), "l");
        assert_eq!(name_for_index(1), "u");
        assert_eq!(name_for_index(2), "c");
        assert_eq!(name_for_index(3), "k");
    }

    #[test]
    fn rolls_over_to_two_chars() {
        assert_eq!(name_for_index(FIRST_CHARS.len() - 1), "_");
        assert_eq!(name_for_index(FIRST_CHARS.len()), "ll");
    }

    #[test]
    fn is_injective_over_the_low_range() {
        let mut seen_names = FxHashSet::default();
        for i in 0..10_000 {
            assert!(seen_names.insert(name_for_index(i)), "collision at {i}");
        }
    }

    #[test]
    fn the_pool_marks_every_two_letter_keyword_reserved() {
        let pick_count = FIRST_CHARS.len() * (REST_CHARS.len() + 1);
        for version in LuaVersion::ALL {
            let allocator = Allocator::new();
            let mut pool = CandidatePool::new(&allocator, version);
            let mut reserved = Vec::new();
            for index in 0..pick_count {
                let (name, is_reserved) = pool.get(index).unwrap();
                if is_reserved {
                    reserved.push(name);
                }
            }
            for keyword in ["do", "if", "in", "or"] {
                assert!(reserved.contains(&keyword), "{keyword} {version:?}");
            }
            assert!(!reserved.contains(&"l"), "{version:?}");
        }
    }

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
        assert!(claims.is_claimed(9, 0));
        assert!(!claims.is_claimed(9, 1));
    }

    #[test]
    fn first_free_crosses_from_the_bits_into_the_set() {
        let mut claims = ScopeClaims::default();
        claims.reset(2);
        for index in (0..70).rev() {
            claims.claim_in(1, index);
        }
        assert_eq!(claims.first_free(1), 70);
        claims.claim_in(1, 71);
        claims.claim_in(1, 70);
        assert_eq!(claims.first_free(1), 72);
        assert!(claims.is_claimed(1, 63) && claims.is_claimed(1, 64));
        assert!(!claims.is_claimed(0, 64));
    }
}
