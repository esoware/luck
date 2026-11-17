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
/// is `CandidatePool`'s job.
fn push_name_for_index(index: usize, name: &mut String) {
    let Some(multi_char_index) = index.checked_sub(FIRST_CHARS.len()) else {
        name.extend(FIRST_CHARS.get(index).copied().map(char::from));
        return;
    };
    let rest_base = REST_CHARS.len();
    push_name_for_index(multi_char_index.div_euclid(rest_base), name);
    name.extend(REST_CHARS.get(multi_char_index.rem_euclid(rest_base)).copied().map(char::from));
}

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

    fn is_claimed(&self, scope: usize, index: usize) -> bool {
        self.claimed.contains(&(scope, index))
    }

    pub(super) fn claim(&mut self, scopes: &[usize], index: usize) {
        for &scope in scopes {
            self.claimed.insert((scope, index));
            let slot = scope;
            if slot >= self.first_free_by_scope.len() {
                self.first_free_by_scope.resize(slot.saturating_add(1), 0);
            }
            if let Some(first_free) = self.first_free_by_scope.get_mut(slot) {
                while self.claimed.contains(&(scope, *first_free)) {
                    *first_free = first_free.saturating_add(1);
                }
            }
        }
    }
}

struct Candidate<'ast> {
    name: &'ast str,
    is_reserved: bool,
}

/// Each name and its keyword check are computed once and shared by every
/// slot.
pub(super) struct CandidatePool<'ast> {
    allocator: &'ast Allocator,
    version: LuaVersion,
    candidates: Vec<Candidate<'ast>>,
    /// Per scope, the candidates no slot live there can take: one a slot
    /// already wears, or the name of a global read there.
    unavailable: ScopeClaims,
    scratch: String,
}

impl<'ast> CandidatePool<'ast> {
    pub(super) fn new(allocator: &'ast Allocator, version: LuaVersion) -> Self {
        Self {
            allocator,
            version,
            candidates: Vec::new(),
            unavailable: ScopeClaims::default(),
            scratch: String::new(),
        }
    }

    /// The first candidate that no slot live in one of `live_scopes` wears,
    /// that isn't a keyword, and that `reads_global` doesn't find read in one
    /// of them. Slots that are never live in the same scope can share a name.
    pub(super) fn pick(
        &mut self,
        live_scopes: &[usize],
        reads_global: impl Fn(usize, &str) -> bool,
    ) -> &'ast str {
        // Every candidate below a scope's first free one is unavailable there.
        let mut index =
            live_scopes.iter().map(|&scope| self.unavailable.first_free(scope)).max().unwrap_or(0);
        loop {
            while self.candidates.len() <= index {
                self.scratch.clear();
                push_name_for_index(self.candidates.len(), &mut self.scratch);
                let is_reserved = TokenKind::is_reserved(&self.scratch, self.version);
                let name = self.allocator.alloc_str(&self.scratch);
                self.candidates.push(Candidate { name, is_reserved });
            }
            let Some(candidate) = self.candidates.get(index) else {
                return "";
            };
            // A keyword or a global's name is recorded where it rules the
            // candidate out, so that scope's first free one moves past it.
            if candidate.is_reserved {
                self.unavailable.claim(live_scopes, index);
            } else if let Some(scope) = live_scopes.iter().copied().find(|&scope| {
                self.unavailable.is_claimed(scope, index) || reads_global(scope, candidate.name)
            }) {
                self.unavailable.claim(&[scope], index);
            } else {
                let name = candidate.name;
                self.unavailable.claim(live_scopes, index);
                return name;
            }
            index = index.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::iter::repeat_with;

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
    fn pick_skips_reserved_words() {
        // Every one- and two-character name, so each two-letter keyword
        // comes up.
        let pick_count = FIRST_CHARS.len() * (REST_CHARS.len() + 1);
        for version in LuaVersion::ALL {
            let allocator = Allocator::new();
            let mut pool = CandidatePool::new(&allocator, version);
            let names: FxHashSet<&str> =
                repeat_with(|| pool.pick(&[0], |_, _| false)).take(pick_count).collect();
            assert_eq!(names.len(), pick_count, "{version:?}");
            for keyword in ["do", "if", "in", "or"] {
                assert!(!names.contains(keyword), "{keyword} {version:?}");
            }
        }
    }

    #[test]
    fn slots_that_share_no_scope_share_a_name() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        assert_eq!(pool.pick(&[0, 1], |_, _| false), "l");
        assert_eq!(pool.pick(&[1, 2], |_, _| false), "u");
        assert_eq!(pool.pick(&[2, 3], |_, _| false), "l");
        assert_eq!(pool.pick(&[4], |_, _| false), "l");
        assert_eq!(pool.pick(&[0, 4], |_, _| false), "u");
    }

    #[test]
    fn pick_skips_a_global_read_in_any_live_scope() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        let reads_global = |scope: usize, name: &str| scope == 3 && (name == "l" || name == "c");
        assert_eq!(pool.pick(&[1, 3], reads_global), "u");
        assert_eq!(pool.pick(&[3], reads_global), "k");
        assert_eq!(pool.pick(&[1], reads_global), "l");
    }

    #[test]
    fn a_pick_among_many_live_slots_starts_past_the_names_they_wear() {
        let allocator = Allocator::new();
        let mut pool = CandidatePool::new(&allocator, LuaVersion::Lua54);
        for _ in 0..5_000u32 {
            pool.pick(&[0], |_, _| false);
        }
        // Without each scope's first free candidate, every pick rescans the
        // names before it.
        assert!(pool.unavailable.first_free(0) >= 5_000);
    }

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
