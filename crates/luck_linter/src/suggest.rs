//! "Did you mean" support shared by the rules that suggest a correction
//! for a mistyped name (`comment_directive`, `invalid_lint_filter`).
//!
//! Only the distance metric is shared; each rule keeps its own acceptance
//! threshold, since what counts as a plausible typo differs by domain.

/// Iterative two-row Levenshtein edit distance. The inputs here are short
/// (rule names, directive keywords), so the quadratic cost is irrelevant.
pub(crate) fn levenshtein(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.is_empty() {
        return b_bytes.len();
    }
    if b_bytes.is_empty() {
        return a_bytes.len();
    }
    let mut prev: Vec<usize> = (0..=b_bytes.len()).collect();
    let mut curr: Vec<usize> = vec![0; b_bytes.len() + 1];
    for i in 1..=a_bytes.len() {
        curr[0] = i;
        for j in 1..=b_bytes.len() {
            let cost = usize::from(a_bytes[i - 1] != b_bytes[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_bytes.len()]
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
