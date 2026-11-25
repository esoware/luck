use luck_allocator::Allocator;
use luck_semantic::Scoping;
use luck_token::TokenKind;

use super::Candidate;

/// Every character a name can hold, the most frequent in minified Luau first.
///
/// Names drawn from it share their bytes with the code around them, so they
/// compress best, as with `oxc_mangler`'s base54 alphabet for JavaScript.
/// The order counts each of these characters in luck's minified output for
/// the hand-written benchmark files and the conformance suites, with every
/// name the renamer gives out printed as `$`, and puts the digits, which
/// can't start a name, last.
const ALPHABET: &str = "etnaroslidcufpgbmhxCyw_TvkDPSFEANIMBRqOLUzGXVHKjWYZJQ1023456897";

/// How many characters of [`ALPHABET`] can start a name.
const FIRST_CHARS: usize = 53;

/// How many characters of [`ALPHABET`] can follow the first.
const REST_CHARS: usize = 63;

/// Writes the name at `index` in the order the renamer gives names out, as
/// `oxc_mangler`'s `base54` does: shortest first, and within a length, with
/// the most frequent characters first.
///
/// The first character is `index` in base 53, and the rest are what's left
/// in bijective base 63, so 53 names have one character, the next 53 * 63
/// have two, and so on. Keywords are among them, which the pool skips.
fn push_base53(index: usize, name: &mut String) {
    let alphabet = ALPHABET.as_bytes();
    name.push(char::from(alphabet[index % FIRST_CHARS]));
    let mut rest = index / FIRST_CHARS;
    while let Some(digit) = rest.checked_sub(1) {
        name.push(char::from(alphabet[digit % REST_CHARS]));
        rest = digit / REST_CHARS;
    }
}

/// Each name but a keyword in turn, computed once and shared by every slot.
pub(super) struct CandidatePool<'walk, 'ast> {
    allocator: &'ast Allocator,
    scoping: &'walk Scoping,
    candidates: Vec<Candidate<'ast>>,
    /// The index of the next name [`push_base53`] writes.
    next_name: usize,
    scratch: String,
}

impl<'walk, 'ast> CandidatePool<'walk, 'ast> {
    pub(super) const fn new(allocator: &'ast Allocator, scoping: &'walk Scoping) -> Self {
        Self { allocator, scoping, candidates: Vec::new(), next_name: 0, scratch: String::new() }
    }

    pub(super) fn get(&mut self, index: usize) -> Candidate<'ast> {
        while self.candidates.len() <= index {
            let next = self.next_name;
            self.next_name += 1;
            // No keyword is one character long.
            let name = if next < FIRST_CHARS {
                &ALPHABET[next..=next]
            } else {
                self.scratch.clear();
                push_base53(next, &mut self.scratch);
                if TokenKind::is_reserved(&self.scratch) {
                    continue;
                }
                self.allocator.alloc_str(&self.scratch)
            };
            self.candidates.push(Candidate { name, name_id: self.scoping.name_id(name) });
        }
        self.candidates[index]
    }
}

#[cfg(test)]
mod tests {
    use rustc_hash::FxHashSet;

    use super::*;
    use crate::binding::analyze;

    fn base53(index: usize) -> String {
        let mut name = String::new();
        push_base53(index, &mut name);
        name
    }

    #[test]
    fn names_count_up_from_the_most_frequent_character() {
        assert_eq!(base53(0), "e");
        assert_eq!(base53(1), "t");
        assert_eq!(base53(FIRST_CHARS - 1), "Q");
        assert_eq!(base53(FIRST_CHARS), "ee");
        assert_eq!(base53(FIRST_CHARS + 1), "te");
        assert_eq!(base53(FIRST_CHARS * 2), "et");
        assert_eq!(base53(FIRST_CHARS * (REST_CHARS + 1) - 1), "Q7");
        assert_eq!(base53(FIRST_CHARS * (REST_CHARS + 1)), "eee");
    }

    #[test]
    fn the_alphabet_holds_every_name_character_once() {
        let mut characters: Vec<u8> = ALPHABET.bytes().collect();
        assert!(characters.iter().take(FIRST_CHARS).all(|b| !b.is_ascii_digit()));
        characters.sort_unstable();
        characters.dedup();
        assert_eq!(characters.len(), REST_CHARS);
        assert!(characters.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_'));
    }

    #[test]
    fn is_injective_over_the_low_range() {
        let mut seen_names = FxHashSet::default();
        for i in 0..100_000 {
            assert!(seen_names.insert(base53(i)), "collision at {i}");
        }
    }

    #[test]
    fn the_pool_skips_every_keyword() {
        let allocator = Allocator::new();
        let result = luck_parser::parse(&allocator, b"");
        let scoping = analyze(&result.chunk, result.counts, None);
        let mut pool = CandidatePool::new(&allocator, &scoping);
        let two_letter_keywords = ["do", "if", "in", "or"];
        let up_to_two_letters = FIRST_CHARS * (REST_CHARS + 1);
        let names: Vec<&str> = (0..up_to_two_letters - two_letter_keywords.len())
            .map(|index| pool.get(index).name)
            .collect();
        assert!(names.iter().all(|name| !TokenKind::is_reserved(name)), "{names:?}");
        assert_eq!(names.last().copied(), Some(base53(up_to_two_letters - 1).as_str()));
        let mut skipped: Vec<String> = (0..up_to_two_letters)
            .map(base53)
            .filter(|name| !names.contains(&name.as_str()))
            .collect();
        skipped.sort_unstable();
        assert_eq!(skipped, two_letter_keywords);
    }
}
