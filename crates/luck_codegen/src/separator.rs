//! Token-boundary ambiguity rules for compact output.
//!
//! The printer remembers the last emitted piece as a one-byte [`PieceClass`]
//! and inserts a space only when that class and the next piece's first byte
//! would otherwise merge into a different token.

/// What the merge rules need to know about an emitted piece. Everything they
/// don't single out is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PieceClass {
    /// Identifier or keyword.
    Word,
    /// Number literal: word-like, and a following `.` would extend it.
    Number,
    Minus,
    Slash,
    LeftBracket,
    Dot,
    DotDot,
    Less,
    Greater,
    /// `;`, tracked so the statement-boundary guard doesn't add a second one.
    Semicolon,
    /// Any other piece, and the start of output, which nothing merges with.
    Other,
}

impl PieceClass {
    /// Classifies a fixed-spelling piece (keyword, operator, punctuation).
    #[inline]
    pub(crate) fn of_fixed(text: &str) -> Self {
        match text {
            "-" => Self::Minus,
            "/" => Self::Slash,
            "[" => Self::LeftBracket,
            "." => Self::Dot,
            ".." => Self::DotDot,
            "<" => Self::Less,
            ">" => Self::Greater,
            ";" => Self::Semicolon,
            _ => match text.as_bytes().first() {
                Some(first) if first.is_ascii_alphanumeric() || *first == b'_' => Self::Word,
                _ => Self::Other,
            },
        }
    }

    /// Identifiers, keywords, and numbers: two of them in a row merge.
    const fn is_wordlike(self) -> bool {
        match self {
            Self::Word | Self::Number => true,
            Self::Minus
            | Self::Slash
            | Self::LeftBracket
            | Self::Dot
            | Self::DotDot
            | Self::Less
            | Self::Greater
            | Self::Semicolon
            | Self::Other => false,
        }
    }
}

/// Whether a space must separate a `previous` piece from the `next` one,
/// whose first byte is `next_first_byte`.
pub(crate) const fn needs_space(
    previous: PieceClass,
    next_first_byte: u8,
    next: PieceClass,
) -> bool {
    match previous {
        // Two adjacent words, whether identifier, keyword, or number, merge.
        PieceClass::Word => next.is_wordlike(),
        // Number before a word merges; number before `.` extends the literal.
        PieceClass::Number => next.is_wordlike() || next_first_byte == b'.',
        // `-` before `-` - prevents `--` comment.
        PieceClass::Minus => next_first_byte == b'-',
        // `/` before `/` - prevents `//` floor division.
        PieceClass::Slash => next_first_byte == b'/',
        // `[` before `[` (token or long-string literal) - prevents `[[`.
        PieceClass::LeftBracket => next_first_byte == b'[',
        // `.` before `.` - prevents `..`. `..` before `.`, `..`, `...`, or
        // `.5` - prevents `...` / vararg merges.
        PieceClass::Dot | PieceClass::DotDot => next_first_byte == b'.',
        // `<` before `<` - prevents `<<` shift.
        PieceClass::Less => next_first_byte == b'<',
        // `>` before `>` or `=` - prevents `>>` and `>=`.
        PieceClass::Greater => next_first_byte == b'>' || next_first_byte == b'=',
        PieceClass::Semicolon | PieceClass::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs_space_between(previous: &str, next: &str) -> bool {
        needs_space(PieceClass::of_fixed(previous), next.as_bytes()[0], PieceClass::of_fixed(next))
    }

    #[test]
    fn needs_space_between_words() {
        assert!(needs_space(PieceClass::Word, b'x', PieceClass::Word));
        assert!(needs_space_between("if", "true"));
        assert!(needs_space_between("return", "nil"));
        assert!(needs_space_between("local", "x"));
    }

    #[test]
    fn needs_space_after_a_number_before_a_dot_or_number() {
        assert!(needs_space(PieceClass::Number, b'.', PieceClass::Other));
        assert!(needs_space(PieceClass::Number, b'1', PieceClass::Number));
    }

    #[test]
    fn needs_space_between_minuses() {
        assert!(needs_space_between("-", "-"));
        assert!(!needs_space_between("-", "x"));
    }

    #[test]
    fn needs_space_between_slashes() {
        assert!(needs_space_between("/", "/"));
        assert!(needs_space_between("/", "//"));
    }

    #[test]
    fn needs_space_between_left_brackets() {
        assert!(needs_space_between("[", "["));
        // A long-string literal also starts with `[`, so it is the same hazard.
        let string_literal = PieceClass::Other;
        assert!(needs_space(PieceClass::LeftBracket, b'[', string_literal));
        assert!(!needs_space(PieceClass::LeftBracket, b'"', string_literal));
    }

    #[test]
    fn needs_space_between_dots() {
        assert!(needs_space_between(".", "."));
        assert!(needs_space_between(".", ".."));
        assert!(needs_space_between("..", "."));
        assert!(needs_space_between("..", "..."));
        // A number starting with a dot, `.5`, is the same hazard.
        assert!(needs_space(PieceClass::DotDot, b'.', PieceClass::Number));
        assert!(!needs_space(PieceClass::DotDot, b'5', PieceClass::Number));
    }

    #[test]
    fn needs_space_between_angle_brackets_that_would_form_one_token() {
        assert!(needs_space_between("<", "<"));
        assert!(needs_space_between(">", ">"));
        assert!(needs_space_between(">", "="));
        assert!(!needs_space_between("<", "x"));
    }

    #[test]
    fn needs_no_space_between_pieces_that_cannot_merge() {
        assert!(!needs_space(PieceClass::Word, b'(', PieceClass::Other));
        assert!(!needs_space_between(")", "("));
        assert!(!needs_space_between(",", "x"));
        assert!(!needs_space_between(";", "x"));
    }

    #[test]
    fn of_fixed_singles_out_only_the_pieces_the_rules_need() {
        assert_eq!(PieceClass::of_fixed("and"), PieceClass::Word);
        assert_eq!(PieceClass::of_fixed("end"), PieceClass::Word);
        assert_eq!(PieceClass::of_fixed("-"), PieceClass::Minus);
        assert_eq!(PieceClass::of_fixed(".."), PieceClass::DotDot);
        assert_eq!(PieceClass::of_fixed("..."), PieceClass::Other);
        assert_eq!(PieceClass::of_fixed(";"), PieceClass::Semicolon);
        assert_eq!(PieceClass::of_fixed("=="), PieceClass::Other);
    }
}
