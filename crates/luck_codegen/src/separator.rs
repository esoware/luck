use luck_token::TokenKind;

/// Whether whitespace is needed between two adjacent tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Separator {
    /// No separator needed.
    None,
    /// A space is required to prevent token merging or ambiguity.
    Space,
}

/// Determine if whitespace is needed between two adjacent tokens to avoid ambiguity.
pub fn needs_separator(prev: &TokenKind, next: &TokenKind) -> Separator {
    // Word followed by word: identifier/keyword/number adjacent would merge
    if is_word(prev) && is_word(next) {
        return Separator::Space;
    }

    // Number before `.` or `..` - prevents `1.` then `.` ambiguity.
    if is_number(prev)
        && matches!(
            next,
            TokenKind::Dot | TokenKind::DotDot | TokenKind::DotDotDot
        )
    {
        return Separator::Space;
    }

    // `-` before `-` - prevents `--` comment.
    if matches!(prev, TokenKind::Minus) && matches!(next, TokenKind::Minus) {
        return Separator::Space;
    }

    if matches!(prev, TokenKind::Minus) && is_number(next) {
        // `-` before a number could look like it needs separating to stop
        // `- -3` from becoming `--3` (a comment), but numbers in the AST are
        // always positive - unary minus is a separate node - so no
        // separator is needed here.
    }

    // `/` before `/` - prevents `//` which is floor div in some versions.
    if matches!(prev, TokenKind::Slash) && matches!(next, TokenKind::Slash) {
        return Separator::Space;
    }

    // `[` before `[` or `[=` - prevents long bracket string `[[`.
    if matches!(prev, TokenKind::LeftBracket) && matches!(next, TokenKind::LeftBracket) {
        return Separator::Space;
    }

    // `[` before a string literal starting with `[` - prevents `[[[...]]` being parsed as
    // `[` then long string `[[...]]`, eating the bracket field's closing `]`.
    if matches!(prev, TokenKind::LeftBracket) {
        if let TokenKind::StringLiteral(s) = next {
            if s.starts_with('[') {
                return Separator::Space;
            }
        }
    }

    // `.` before `.` - prevents `..`.
    if matches!(prev, TokenKind::Dot) && matches!(next, TokenKind::Dot | TokenKind::DotDot) {
        return Separator::Space;
    }

    // `<` before `<` - prevents `<<` shift.
    if matches!(prev, TokenKind::Less) && matches!(next, TokenKind::Less) {
        return Separator::Space;
    }

    // `>` before `>` - prevents `>>` shift.
    if matches!(prev, TokenKind::Greater) && matches!(next, TokenKind::Greater) {
        return Separator::Space;
    }

    // `>` before `=` - prevents `>=` (attribute close `>` followed by assignment `=`).
    if matches!(prev, TokenKind::Greater) && matches!(next, TokenKind::Equal) {
        return Separator::Space;
    }

    // Note: `~=`, `==`, `<=` are each lexed as single tokens and don't occur
    // as separate token pairs in AST output. `>=` can occur when attribute
    // close `>` is followed by `=` in local assignments.

    // DotDot before `.` or DotDot - prevents `...` or `....`.
    if matches!(prev, TokenKind::DotDot)
        && matches!(
            next,
            TokenKind::Dot | TokenKind::DotDot | TokenKind::DotDotDot
        )
    {
        return Separator::Space;
    }

    // Number before DotDot is already handled above (number before `.`/`..`)

    // DotDot before a number starting with `.` - prevents `..` + `.5` -> `...5` (vararg + 5).
    if matches!(prev, TokenKind::DotDot) {
        if let TokenKind::Number(n) = next {
            if n.starts_with('.') {
                return Separator::Space;
            }
        }
    }

    Separator::None
}

fn is_word(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier(_)
            | TokenKind::Number(_)
            | TokenKind::And
            | TokenKind::Break
            | TokenKind::Do
            | TokenKind::Else
            | TokenKind::ElseIf
            | TokenKind::End
            | TokenKind::False
            | TokenKind::For
            | TokenKind::Function
            | TokenKind::Goto
            | TokenKind::Global
            | TokenKind::If
            | TokenKind::In
            | TokenKind::Local
            | TokenKind::Nil
            | TokenKind::Not
            | TokenKind::Or
            | TokenKind::Repeat
            | TokenKind::Return
            | TokenKind::Then
            | TokenKind::True
            | TokenKind::Until
            | TokenKind::While
    )
}

fn is_number(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Number(_))
}

#[cfg(test)]
mod tests {
    use luck_token::TokenKind;

    use super::{Separator, needs_separator};

    #[test]
    fn word_word_needs_space() {
        let ident = TokenKind::Identifier("x".into());
        assert_eq!(needs_separator(&ident, &ident), Separator::Space);
    }

    #[test]
    fn keyword_keyword_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::If, &TokenKind::True),
            Separator::Space
        );
        assert_eq!(
            needs_separator(&TokenKind::Return, &TokenKind::Nil),
            Separator::Space
        );
    }

    #[test]
    fn keyword_identifier_needs_space() {
        let ident = TokenKind::Identifier("x".into());
        assert_eq!(needs_separator(&TokenKind::Local, &ident), Separator::Space);
        assert_eq!(
            needs_separator(&TokenKind::Return, &ident),
            Separator::Space
        );
    }

    #[test]
    fn number_dot_needs_space() {
        let num = TokenKind::Number("1".into());
        assert_eq!(needs_separator(&num, &TokenKind::Dot), Separator::Space);
        assert_eq!(needs_separator(&num, &TokenKind::DotDot), Separator::Space);
    }

    #[test]
    fn minus_minus_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::Minus, &TokenKind::Minus),
            Separator::Space
        );
    }

    #[test]
    fn slash_slash_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::Slash, &TokenKind::Slash),
            Separator::Space
        );
    }

    #[test]
    fn bracket_bracket_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::LeftBracket, &TokenKind::LeftBracket),
            Separator::Space
        );
    }

    #[test]
    fn dot_dot_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::Dot, &TokenKind::Dot),
            Separator::Space
        );
    }

    #[test]
    fn shift_prevention() {
        assert_eq!(
            needs_separator(&TokenKind::Less, &TokenKind::Less),
            Separator::Space
        );
        assert_eq!(
            needs_separator(&TokenKind::Greater, &TokenKind::Greater),
            Separator::Space
        );
    }

    #[test]
    fn symbol_no_space() {
        let ident = TokenKind::Identifier("x".into());
        assert_eq!(
            needs_separator(&ident, &TokenKind::LeftParen),
            Separator::None
        );
        assert_eq!(
            needs_separator(&TokenKind::RightParen, &TokenKind::LeftParen),
            Separator::None
        );
        assert_eq!(needs_separator(&TokenKind::Comma, &ident), Separator::None);
    }

    // Note: `==`, `<=`, `>=`, `~=` are single tokens from the lexer,
    // so those two-token sequences don't occur in AST output.

    #[test]
    fn number_number_needs_space() {
        let num = TokenKind::Number("1".into());
        assert_eq!(needs_separator(&num, &num), Separator::Space);
    }

    #[test]
    fn dotdot_dot_needs_space() {
        assert_eq!(
            needs_separator(&TokenKind::DotDot, &TokenKind::Dot),
            Separator::Space
        );
    }

    #[test]
    fn dotdot_before_dot_number_needs_space() {
        let dot_num = TokenKind::Number(".5".into());
        assert_eq!(
            needs_separator(&TokenKind::DotDot, &dot_num),
            Separator::Space
        );
    }

    #[test]
    fn dotdot_before_plain_number_no_space() {
        let num = TokenKind::Number("5".into());
        assert_eq!(needs_separator(&TokenKind::DotDot, &num), Separator::None);
    }

    #[test]
    fn bracket_before_bracket_string_needs_space() {
        let long_str = TokenKind::StringLiteral("[[key]]".into());
        assert_eq!(
            needs_separator(&TokenKind::LeftBracket, &long_str),
            Separator::Space
        );
    }

    #[test]
    fn bracket_before_quoted_string_no_space() {
        let quoted_str = TokenKind::StringLiteral("\"hello\"".into());
        assert_eq!(
            needs_separator(&TokenKind::LeftBracket, &quoted_str),
            Separator::None
        );
    }
}
