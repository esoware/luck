//! Every piece the printer writes leaves a one-bit class behind, and the
//! next piece's first byte names the classes it would merge with, so a space
//! goes in only where the two meet.
//!
//! The classes cover the merges a tree can print. Only a Luau type prints a
//! `<` or `>` before another, and Luau has no shift, so those stay glued.
//! Every other merge, such as `/` into `//` or `=` into `==`, takes a pair no
//! tree prints.

use std::ops::BitOr;

/// Classes, one bit each: the one a piece leaves, or the ones it merges with.
#[derive(Clone, Copy)]
pub(crate) struct Classes(u8);

impl Classes {
    #[inline]
    pub(crate) const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl BitOr for Classes {
    type Output = Self;

    #[inline]
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

pub(crate) const OTHER: Classes = Classes(0);
/// A keyword or a name.
pub(crate) const WORD: Classes = Classes(1);
/// A number reads on through letters, digits, `_`, and `.`.
pub(crate) const NUMBER: Classes = Classes(1 << 1);
/// `..`, which a `.` extends to `...`, and the field `.`, which nothing that
/// starts with one follows.
pub(crate) const DOT_DOT: Classes = Classes(1 << 2);
/// `-`, which another makes a comment.
pub(crate) const MINUS: Classes = Classes(1 << 3);
/// `[`, which a long string's `[` makes a long bracket.
pub(crate) const LEFT_BRACKET: Classes = Classes(1 << 4);
/// `>`, which a `=` makes `>=`.
pub(crate) const GREATER: Classes = Classes(1 << 5);
/// Luau: the `{` that opens an interpolation, which a table's `{` makes `{{`.
pub(crate) const INTERPOLATION_BRACE: Classes = Classes(1 << 6);

/// The classes a piece starting with `first` merges with.
#[inline]
pub(crate) fn merges_after(first: u8) -> Classes {
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
pub(crate) const fn class_of(last: u8) -> Classes {
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
        let last = previous.as_bytes()[previous.len() - 1];
        let first = next.as_bytes()[0];
        class_of(last).intersects(merges_after(first))
    }

    #[test]
    fn words_and_numbers_merge_with_what_continues_a_name() {
        for (previous, next) in [("if", "true"), ("return", "nil"), ("local", "x"), ("x", "1")] {
            assert!(needs_space_between(previous, next), "{previous} {next}");
        }
        assert!(NUMBER.intersects(merges_after(b'x')));
        assert!(NUMBER.intersects(merges_after(b'.')), "`1..` is a malformed number");
        assert!(!WORD.intersects(merges_after(b'.')), "`return.5` and `a..b` read as written");
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
        assert!(INTERPOLATION_BRACE.intersects(merges_after(b'{')));
        assert!(!needs_space_between("{", "{"), "a table in a table is `{{`");
    }
}
