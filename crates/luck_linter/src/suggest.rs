//! Edit distance for the rules that suggest a fix for a mistyped name.
//!
//! Each rule sets its own threshold, since what counts as a plausible typo
//! differs by domain.

use core::mem;

/// Byte-wise Levenshtein distance. The inputs are short names, so the
/// quadratic cost doesn't matter.
pub(crate) fn levenshtein(left: &str, right: &str) -> usize {
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
}
