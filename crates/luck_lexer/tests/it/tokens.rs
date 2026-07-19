use luck_token::*;

use crate::common::{first_kind, kinds, lex51};

#[test]
fn all_lua51_keywords() {
    let cases = [
        ("and", TokenKind::And),
        ("break", TokenKind::Break),
        ("do", TokenKind::Do),
        ("else", TokenKind::Else),
        ("elseif", TokenKind::ElseIf),
        ("end", TokenKind::End),
        ("false", TokenKind::False),
        ("for", TokenKind::For),
        ("function", TokenKind::Function),
        ("if", TokenKind::If),
        ("in", TokenKind::In),
        ("local", TokenKind::Local),
        ("nil", TokenKind::Nil),
        ("not", TokenKind::Not),
        ("or", TokenKind::Or),
        ("repeat", TokenKind::Repeat),
        ("return", TokenKind::Return),
        ("then", TokenKind::Then),
        ("true", TokenKind::True),
        ("until", TokenKind::Until),
        ("while", TokenKind::While),
    ];
    for (source, expected) in cases {
        assert_eq!(
            first_kind(source),
            expected,
            "failed for keyword: {}",
            source
        );
    }
}

#[test]
fn identifier_simple() {
    assert_eq!(first_kind("foo"), TokenKind::Identifier("foo".into()));
}

#[test]
fn identifier_underscore_prefix() {
    assert_eq!(first_kind("_bar"), TokenKind::Identifier("_bar".into()));
}

#[test]
fn identifier_underscore_digits() {
    assert_eq!(first_kind("_123"), TokenKind::Identifier("_123".into()));
}

#[test]
fn identifier_mixed() {
    assert_eq!(
        first_kind("abc_def"),
        TokenKind::Identifier("abc_def".into())
    );
}

#[test]
fn all_single_char_symbols() {
    let cases = [
        ("+", TokenKind::Plus),
        ("-", TokenKind::Minus),
        ("*", TokenKind::Star),
        ("/", TokenKind::Slash),
        ("%", TokenKind::Percent),
        ("^", TokenKind::Caret),
        // '#' at the very start of a chunk is a first-line comment (like
        // luaL_loadfilex); lex it after a newline to get the length op.
        ("\n#", TokenKind::Hash),
        ("(", TokenKind::LeftParen),
        (")", TokenKind::RightParen),
        ("{", TokenKind::LeftBrace),
        ("}", TokenKind::RightBrace),
        ("[", TokenKind::LeftBracket),
        ("]", TokenKind::RightBracket),
        (";", TokenKind::Semicolon),
        (":", TokenKind::Colon),
        (",", TokenKind::Comma),
        (".x", TokenKind::Dot),
        ("=", TokenKind::Equal),
        ("<", TokenKind::Less),
        (">", TokenKind::Greater),
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for: {:?}", source);
    }
}

#[test]
fn all_multi_char_symbols() {
    let cases = [
        ("==", TokenKind::EqualEqual),
        ("~=", TokenKind::TildeEqual),
        ("<=", TokenKind::LessEqual),
        (">=", TokenKind::GreaterEqual),
        ("..x", TokenKind::DotDot),
        ("...", TokenKind::DotDotDot),
        ("::", TokenKind::DoubleColon),
    ];
    for (source, expected) in cases {
        assert_eq!(first_kind(source), expected, "failed for: {:?}", source);
    }
}

#[test]
fn whitespace_not_stored_as_tokens() {
    let ks = kinds("  x  +  y  ");
    assert_eq!(
        ks,
        vec![
            TokenKind::Identifier("x".into()),
            TokenKind::Plus,
            TokenKind::Identifier("y".into()),
        ]
    );
}

#[test]
fn eof_at_end() {
    let result = lex51("x");
    assert_eq!(
        result
            .tokens
            .last()
            .expect("tokens should not be empty")
            .kind,
        TokenKind::Eof
    );
}

#[test]
fn empty_source_produces_eof() {
    let result = lex51("");
    assert_eq!(result.tokens.len(), 1);
    assert_eq!(result.tokens[0].kind, TokenKind::Eof);
}

#[test]
fn span_correctness() {
    let result = lex51("local x = 42");
    assert_eq!(result.tokens[0].span, Span::new(0, 5));
    assert_eq!(result.tokens[0].kind, TokenKind::Local);
    assert_eq!(result.tokens[1].span, Span::new(6, 7));
    assert_eq!(result.tokens[2].span, Span::new(8, 9));
    assert_eq!(result.tokens[3].span, Span::new(10, 12));
}

#[test]
fn full_statement() {
    let ks = kinds("local x = 1 + 2");
    assert_eq!(
        ks,
        vec![
            TokenKind::Local,
            TokenKind::Identifier("x".into()),
            TokenKind::Equal,
            TokenKind::Number("1".into()),
            TokenKind::Plus,
            TokenKind::Number("2".into()),
        ]
    );
}

#[test]
fn function_call_with_string() {
    let ks = kinds("print(\"hello\")");
    assert_eq!(
        ks,
        vec![
            TokenKind::Identifier("print".into()),
            TokenKind::LeftParen,
            TokenKind::StringLiteral("\"hello\"".into()),
            TokenKind::RightParen,
        ]
    );
}

#[test]
fn table_constructor() {
    let ks = kinds("{1, 2, 3}");
    assert_eq!(
        ks,
        vec![
            TokenKind::LeftBrace,
            TokenKind::Number("1".into()),
            TokenKind::Comma,
            TokenKind::Number("2".into()),
            TokenKind::Comma,
            TokenKind::Number("3".into()),
            TokenKind::RightBrace,
        ]
    );
}

#[test]
fn dot_method_chain() {
    let ks = kinds("a.b:c()");
    assert_eq!(
        ks,
        vec![
            TokenKind::Identifier("a".into()),
            TokenKind::Dot,
            TokenKind::Identifier("b".into()),
            TokenKind::Colon,
            TokenKind::Identifier("c".into()),
            TokenKind::LeftParen,
            TokenKind::RightParen,
        ]
    );
}

#[test]
fn varargs_and_concat() {
    let ks = kinds("... .. .");
    assert_eq!(
        ks,
        vec![TokenKind::DotDotDot, TokenKind::DotDot, TokenKind::Dot,]
    );
}
