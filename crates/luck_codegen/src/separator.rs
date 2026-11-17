//! Every piece the printer writes leaves a one-bit class behind, and the
//! next piece's first byte names the classes it would merge with, so a space
//! goes in only where the two meet.
//!
//! The classes cover the merges a tree can print. Only a Luau type prints a
//! `<` or `>` before another, and Luau has no shift, so those stay glued.
//! Every other merge, such as `/` into `//` or `=` into `==`, takes a pair no
//! tree prints.

pub(crate) const OTHER: u8 = 0;
/// A keyword or a name.
pub(crate) const WORD: u8 = 1;
/// A number reads on through letters, digits, `_`, and `.`.
pub(crate) const NUMBER: u8 = 1 << 1;
/// `..`, which a `.` extends to `...`, and the field `.`, which nothing that
/// starts with one follows.
pub(crate) const DOT_DOT: u8 = 1 << 2;
/// `-`, which another makes a comment.
pub(crate) const MINUS: u8 = 1 << 3;
/// `[`, which a long string's `[` makes a long bracket.
pub(crate) const LEFT_BRACKET: u8 = 1 << 4;
/// `>`, which a `=` makes `>=`.
pub(crate) const GREATER: u8 = 1 << 5;
/// Luau: the `{` that opens an interpolation, which a table's `{` makes `{{`.
pub(crate) const INTERPOLATION_BRACE: u8 = 1 << 6;

/// The classes a piece starting with `first` merges with.
#[inline]
pub(crate) const fn merges_after(first: u8) -> u8 {
    match first {
        b'0'..=b'9' | b'A'..=b'Z' | b'_' | b'a'..=b'z' => WORD | NUMBER,
        b'.' => NUMBER | DOT_DOT,
        b'-' => MINUS,
        b'[' => LEFT_BRACKET,
        b'=' => GREATER,
        b'{' => INTERPOLATION_BRACE,
        _ => OTHER,
    }
}

/// The class a keyword, symbol, operator, or string ending in `last` leaves.
#[inline]
pub(crate) const fn class_of(last: u8) -> u8 {
    match last {
        b'0'..=b'9' | b'A'..=b'Z' | b'_' | b'a'..=b'z' => WORD,
        b'.' => DOT_DOT,
        b'-' => MINUS,
        b'[' => LEFT_BRACKET,
        b'>' => GREATER,
        _ => OTHER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs_space_between(previous: &str, next: &str) -> bool {
        let last = previous.as_bytes().last().copied().unwrap_or_default();
        let first = next.as_bytes().first().copied().unwrap_or_default();
        class_of(last) & merges_after(first) != OTHER
    }

    #[test]
    fn words_and_numbers_merge_with_what_continues_a_name() {
        for (previous, next) in [("if", "true"), ("return", "nil"), ("local", "x"), ("x", "1")] {
            assert!(needs_space_between(previous, next), "{previous} {next}");
        }
        assert_ne!(NUMBER & merges_after(b'x'), OTHER);
        assert_ne!(NUMBER & merges_after(b'.'), OTHER, "`1..` is a malformed number");
        assert_eq!(WORD & merges_after(b'.'), OTHER, "`return.5` and `a..b` read as written");
    }

    #[test]
    fn symbols_merge_only_into_a_longer_token() {
        for (previous, next) in [("-", "-"), ("..", "."), ("..", "..."), ("[", "[["), (">", "=")] {
            assert!(needs_space_between(previous, next), "{previous} {next}");
        }
        for (previous, next) in
            [("-", "x"), ("..", "5"), ("[", "'s'"), ("<", "<"), (">", ">"), ("<", "=")]
        {
            assert!(!needs_space_between(previous, next), "{previous} {next}");
        }
    }

    #[test]
    fn pieces_that_cannot_merge_stay_glued() {
        for (previous, next) in [("x", "("), (")", "("), (",", "x"), (";", "x"), ("]", "]")] {
            assert!(!needs_space_between(previous, next), "{previous} {next}");
        }
    }

    #[test]
    fn only_an_interpolation_brace_merges_with_a_table() {
        assert_ne!(INTERPOLATION_BRACE & merges_after(b'{'), OTHER);
        assert!(!needs_space_between("{", "{"), "a table in a table is `{{`");
    }
}
