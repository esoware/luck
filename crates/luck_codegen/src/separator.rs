//! Token-boundary ambiguity rules for compact output.
//!
//! The printer tracks the previously emitted piece as a one-byte
//! [`PrevClass`] instead of a cloned `TokenKind`; a space is inserted
//! only when the previous class and the next piece's first byte would
//! otherwise merge into a different token.

/// Class of the last emitted piece. Only the distinctions the merge
/// rules need are represented; everything else is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrevClass {
    None,
    /// Identifier or keyword.
    Word,
    /// Number literal (word-like, and `1.` + `.` would extend it).
    Number,
    Minus,
    Slash,
    LeftBracket,
    Dot,
    DotDot,
    Less,
    Greater,
    /// `;` - tracked so the statement-boundary guard can tell a fresh
    /// semicolon apart without storing a token.
    Semicolon,
    Other,
}

/// Classify a fixed-spelling piece (keyword, operator, punctuation). A
/// piece is word-like exactly when this returns [`PrevClass::Word`], so the
/// caller derives word-likeness from the class rather than rescanning.
pub fn classify_str(text: &str) -> PrevClass {
    match text {
        "-" => PrevClass::Minus,
        "/" => PrevClass::Slash,
        "[" => PrevClass::LeftBracket,
        "." => PrevClass::Dot,
        ".." => PrevClass::DotDot,
        "<" => PrevClass::Less,
        ">" => PrevClass::Greater,
        ";" => PrevClass::Semicolon,
        _ => match text.as_bytes().first() {
            Some(first) if first.is_ascii_alphanumeric() || *first == b'_' => PrevClass::Word,
            _ => PrevClass::Other,
        },
    }
}

/// Whether a space must separate the previous piece from the next one.
/// `next_first` is the next piece's first byte; `next_is_wordlike` is true
/// for identifiers, keywords, and numbers.
pub fn needs_space(prev: PrevClass, next_first: u8, next_is_wordlike: bool) -> bool {
    match prev {
        // Word followed by word: identifier/keyword/number adjacent would merge.
        PrevClass::Word => next_is_wordlike,
        // Number before a word merges; number before `.` extends the literal.
        PrevClass::Number => next_is_wordlike || next_first == b'.',
        // `-` before `-` - prevents `--` comment.
        PrevClass::Minus => next_first == b'-',
        // `/` before `/` - prevents `//` floor division.
        PrevClass::Slash => next_first == b'/',
        // `[` before `[` (token or long-string literal) - prevents `[[`.
        PrevClass::LeftBracket => next_first == b'[',
        // `.` before `.` - prevents `..`.
        PrevClass::Dot => next_first == b'.',
        // `..` before `.`, `..`, `...`, or `.5` - prevents `...` / vararg merges.
        PrevClass::DotDot => next_first == b'.',
        // `<` before `<` - prevents `<<` shift.
        PrevClass::Less => next_first == b'<',
        // `>` before `>` or `=` - prevents `>>` and `>=`.
        PrevClass::Greater => next_first == b'>' || next_first == b'=',
        PrevClass::None | PrevClass::Semicolon | PrevClass::Other => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sep_str(prev: &str, next: &str) -> bool {
        let next_is_wordlike = matches!(classify_str(next), PrevClass::Word);
        needs_space(classify_str(prev), next.as_bytes()[0], next_is_wordlike)
    }

    fn sep_after_word(next_first: u8, next_is_wordlike: bool) -> bool {
        needs_space(PrevClass::Word, next_first, next_is_wordlike)
    }

    #[test]
    fn word_word_needs_space() {
        assert!(sep_after_word(b'x', true));
        assert!(sep_str("if", "true"));
        assert!(sep_str("return", "nil"));
        assert!(sep_str("local", "x"));
    }

    #[test]
    fn number_dot_needs_space() {
        assert!(needs_space(PrevClass::Number, b'.', false));
        // Number before number (word-like) needs a space too.
        assert!(needs_space(PrevClass::Number, b'1', true));
        // `..` before `.5` - number starting with a dot.
        assert!(needs_space(PrevClass::DotDot, b'.', true));
        // `..` before a plain number does not.
        assert!(!needs_space(PrevClass::DotDot, b'5', true));
    }

    #[test]
    fn minus_minus_needs_space() {
        assert!(sep_str("-", "-"));
        assert!(!sep_str("-", "x"));
    }

    #[test]
    fn slash_slash_needs_space() {
        assert!(sep_str("/", "/"));
        assert!(sep_str("/", "//"));
    }

    #[test]
    fn bracket_bracket_needs_space() {
        assert!(sep_str("[", "["));
        // `[` before a long-string literal starting with `[`.
        assert!(needs_space(PrevClass::LeftBracket, b'[', false));
        // `[` before a quoted string does not.
        assert!(!needs_space(PrevClass::LeftBracket, b'"', false));
    }

    #[test]
    fn dot_dot_needs_space() {
        assert!(sep_str(".", "."));
        assert!(sep_str(".", ".."));
        assert!(sep_str("..", "."));
        assert!(sep_str("..", "..."));
    }

    #[test]
    fn shift_prevention() {
        assert!(sep_str("<", "<"));
        assert!(sep_str(">", ">"));
        assert!(sep_str(">", "="));
        assert!(!sep_str("<", "x"));
    }

    #[test]
    fn symbol_no_space() {
        assert!(!sep_after_word(b'(', false));
        assert!(!sep_str(")", "("));
        assert!(!sep_str(",", "x"));
        assert!(!sep_str(";", "x"));
    }

    #[test]
    fn classification() {
        assert_eq!(classify_str("and"), PrevClass::Word);
        assert_eq!(classify_str("end"), PrevClass::Word);
        assert_eq!(classify_str("-"), PrevClass::Minus);
        assert_eq!(classify_str(".."), PrevClass::DotDot);
        assert_eq!(classify_str("..."), PrevClass::Other);
        assert_eq!(classify_str(";"), PrevClass::Semicolon);
        assert_eq!(classify_str("=="), PrevClass::Other);
    }
}
