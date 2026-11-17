use luck_allocator::Allocator;
use luck_token::{LuaVersion, TokenKind};

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
    use rustc_hash::FxHashSet;

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
}
