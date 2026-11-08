//! Short names for the renamer's slots, shortest first.

use luck_token::{CompactString, LuaVersion, TokenKind};
use rustc_hash::FxHashSet;

use super::scope_set::ScopeSet;

const FIRST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyzABDEFGHIJMNOPQRSTVWXYZ_";
const REST_CHARS: &[u8] = b"luckLUCKabdefghijmnopqrstvwxyz_ABDEFGHIJMNOPQRSTVWXYZ0123456789";

/// The `index`-th candidate identifier, shortest first.
///
/// The first character comes from `FIRST_CHARS` (53 of them) and each later
/// one from `REST_CHARS` (63, digits included). Indices 0 through 52 are one
/// character, the next 53 * 63 are two, the next 53 * 63^2 are three, and so
/// on. Past the first 53, index `53 + n` is name `n / 63` followed by
/// `REST_CHARS[n % 63]`.
///
/// The sequence includes keywords. Skipping keywords and names already in use
/// is `CandidatePool`'s job.
fn name_for_index(index: usize) -> CompactString {
    let Some(multi_char_index) = index.checked_sub(FIRST_CHARS.len()) else {
        return char_at(FIRST_CHARS, index).into_iter().collect();
    };
    let rest_base = REST_CHARS.len();
    let mut name = name_for_index(multi_char_index.div_euclid(rest_base));
    name.extend(char_at(REST_CHARS, multi_char_index.rem_euclid(rest_base)));
    name
}

fn char_at(alphabet: &[u8], index: usize) -> Option<char> {
    alphabet.get(index).copied().map(char::from)
}

struct Candidate {
    name: CompactString,
    is_reserved: bool,
    // Scopes where some slot already wears the name. Slots that are never
    // live in the same scope can share it.
    taken_scopes: ScopeSet,
}

impl Candidate {
    fn is_available(&self, live_scopes: &ScopeSet, forbidden_names: &FxHashSet<&str>) -> bool {
        !live_scopes.intersects(&self.taken_scopes)
            && !self.is_reserved
            && !forbidden_names.contains(self.name.as_str())
    }

    fn claim(&mut self, live_scopes: &ScopeSet) -> CompactString {
        self.taken_scopes.union_with(live_scopes);
        self.name.clone()
    }
}

/// Shared candidate-name state for `Analyzer::generate_names`.
///
/// Every slot scans candidates from index 0, so each name and its keyword
/// check are computed once and shared.
pub(super) struct CandidatePool {
    version: LuaVersion,
    candidates: Vec<Candidate>,
}

impl CandidatePool {
    pub(super) const fn new(version: LuaVersion) -> Self {
        Self { version, candidates: Vec::new() }
    }

    pub(super) fn pick(
        &mut self,
        live_scopes: &ScopeSet,
        forbidden_names: &FxHashSet<&str>,
    ) -> CompactString {
        if let Some(candidate) = self
            .candidates
            .iter_mut()
            .find(|candidate| candidate.is_available(live_scopes, forbidden_names))
        {
            return candidate.claim(live_scopes);
        }
        loop {
            let name = name_for_index(self.candidates.len());
            let is_reserved = TokenKind::is_reserved(&name, self.version);
            let mut candidate = Candidate { name, is_reserved, taken_scopes: ScopeSet::default() };
            let claimed_name = candidate
                .is_available(live_scopes, forbidden_names)
                .then(|| candidate.claim(live_scopes));
            self.candidates.push(candidate);
            if let Some(claimed) = claimed_name {
                return claimed;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::iter::repeat_with;

    use super::*;

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
        for index in 0..10_000 {
            assert!(seen_names.insert(name_for_index(index)), "collision at {index}");
        }
    }

    #[test]
    fn pick_skips_reserved_words() {
        let mut live_scopes = ScopeSet::default();
        live_scopes.insert(0);
        let forbidden_names = FxHashSet::default();
        // Every one- and two-character name, so each two-letter keyword
        // comes up.
        let pick_count = FIRST_CHARS.len() * (REST_CHARS.len() + 1);
        for version in LuaVersion::ALL {
            let mut pool = CandidatePool::new(version);
            let names: FxHashSet<CompactString> =
                repeat_with(|| pool.pick(&live_scopes, &forbidden_names))
                    .take(pick_count)
                    .collect();
            assert_eq!(names.len(), pick_count, "{version:?}");
            for keyword in ["do", "if", "in", "or"] {
                assert!(!names.contains(keyword), "{keyword} {version:?}");
            }
        }
    }
}
