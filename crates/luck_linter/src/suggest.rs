//! Each rule sets its own threshold, since what counts as a plausible typo
//! differs by domain.

use core::mem;

/// The inputs are short names, so the quadratic cost doesn't matter.
fn levenshtein(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    let right_bytes = right.as_bytes();
    let mut distance = right_bytes.len();
    let mut previous_row: Vec<usize> = (0..=right_bytes.len()).collect();
    let mut current_row: Vec<usize> = Vec::with_capacity(previous_row.len());
    for (row, left_byte) in (1..).zip(left.bytes()) {
        current_row.clear();
        current_row.push(row);
        distance = row;
        for ((right_byte, &diagonal), &above) in
            right_bytes.iter().zip(&previous_row).zip(previous_row.iter().skip(1))
        {
            let edit = above.min(distance).saturating_add(1);
            let substitution = diagonal.saturating_add(usize::from(left_byte != *right_byte));
            distance = edit.min(substitution);
            current_row.push(distance);
        }
        mem::swap(&mut previous_row, &mut current_row);
    }
    distance
}

/// The candidate nearest to `needle`, among those `is_close` accepts at
/// their distance. The first of equally near candidates wins.
pub(crate) fn closest<'candidate>(
    needle: &str,
    candidates: impl IntoIterator<Item = &'candidate str>,
    is_close: impl Fn(usize, &str) -> bool,
) -> Option<&'candidate str> {
    candidates
        .into_iter()
        .map(|candidate| (levenshtein(needle, candidate), candidate))
        .filter(|&(distance, candidate)| is_close(distance, candidate))
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, candidate)| candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_matches_known_pairs() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "ab"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn closest_picks_nearest_candidate() {
        assert_eq!(closest("abcd", ["abxy", "abcx"], |distance, _| distance <= 2), Some("abcx"));
    }

    #[test]
    fn closest_picks_first_of_equally_near_candidates() {
        assert_eq!(closest("abc", ["abd", "abe"], |distance, _| distance <= 2), Some("abd"));
    }

    #[test]
    fn closest_skips_candidates_it_rejects() {
        let is_close = |distance: usize, candidate: &str| candidate != "abd" && distance <= 3;
        assert_eq!(closest("abc", ["abd", "xyz"], is_close), Some("xyz"));
        assert!(closest("abc", ["abd"], is_close).is_none());
    }
}
