//! `luck_token`'s literal predicates say whether a text reads back as one
//! token for printers that can't run the lexer, so they have to agree with it
//! exactly.

use luck_lexer::lex;
use luck_token::{
    LuaVersion, TokenKind, is_interpolated_text, is_numeral_in, is_string_literal_in,
};

/// `starts`, and every text that continues one of them with up to
/// `max_extra` bytes from `alphabet`.
fn texts(starts: &[&str], alphabet: &[u8], max_extra: usize) -> Vec<String> {
    let mut frontier: Vec<String> = starts.iter().map(|start| (*start).to_owned()).collect();
    let mut all = frontier.clone();
    for _ in 0..max_extra {
        frontier = frontier
            .iter()
            .flat_map(|text| {
                alphabet.iter().map(move |&b| {
                    let mut longer = text.clone();
                    longer.push(char::from(b));
                    longer
                })
            })
            .collect();
        all.extend(frontier.iter().cloned());
    }
    all
}

/// Asserts that `is_token` agrees with the lexer on every candidate, and that
/// each answer comes up, so the comparison can't pass by refusing everything.
fn assert_agrees(
    candidates: &[String],
    versions: &[LuaVersion],
    is_token: impl Fn(&str, LuaVersion) -> bool,
    tokens: impl Fn(&str) -> (String, Vec<TokenKind>),
) {
    let mut accepted_count = 0usize;
    let mut checked_count = 0usize;
    for &version in versions {
        for text in candidates {
            let (source, expected) = tokens(text);
            let is_accepted = is_token(text, version);
            assert_eq!(
                is_accepted,
                lexes_as(&source, version, &expected),
                "{text:?} under {version:?}"
            );
            accepted_count = accepted_count.saturating_add(usize::from(is_accepted));
            checked_count = checked_count.saturating_add(1);
        }
    }
    assert!(
        accepted_count > 0 && accepted_count < checked_count,
        "{accepted_count} of {checked_count}"
    );
}

/// Whether `source` lexes clean under `version` as exactly the `tokens`.
fn lexes_as(source: &str, version: LuaVersion, tokens: &[TokenKind]) -> bool {
    let result = lex(source, version);
    let kinds: Vec<&TokenKind> = result.tokens.iter().map(|token| &token.kind).collect();
    result.errors.is_empty()
        && kinds.split_last().is_some_and(|(eof, rest)| {
            **eof == TokenKind::Eof && rest.iter().copied().eq(tokens.iter())
        })
}

#[test]
fn the_numeral_predicate_agrees_with_the_lexer() {
    let mut candidates = texts(&["0", "1", ".", "0x", "0b", "0_"], b"019aefpxbiEPXB_.+-", 3);
    candidates.extend(
        [
            "0x1.8p3",
            "0x.8p-1",
            "0x1P+4",
            "0xA.Bp1i",
            "1.5e+10",
            "3.14e-2",
            "1_000_000",
            "1__2_",
            "0b1010_1010i",
            "0xffffffffffffffffi",
            "0x1_0000_0000_0000_0000i",
            "9223372036854775807i",
            "9223372036854775808i",
            "18446744073709551616",
            "1e5..",
            "0xFF..",
        ]
        .map(str::to_owned),
    );
    assert_agrees(&candidates, &LuaVersion::ALL, is_numeral_in, |text| {
        (text.to_owned(), vec![TokenKind::Number(text.into())])
    });
}

#[test]
fn the_string_predicate_agrees_with_the_lexer() {
    let mut candidates = texts(&["\"", "'", "[[", "[=["], b"\"'\\nzxu{}0259a[]=\n\r \0", 3);
    candidates.extend(
        [
            "\"\\x41\"",
            "\"\\x4g\"",
            "\"\\u{10FFFF}\"",
            "\"\\u{110000}\"",
            "\"\\u{7FFFFFFF}\"",
            "\"\\u{80000000}\"",
            "\"\\u{00000000000000041}\"",
            "\"\\u{}\"",
            "\"\\u{41\"",
            "\"\\z  \n\t x\"",
            "\"\\255\"",
            "\"\\256\"",
            "\"\\0019\"",
            "\"a\\\n\rb\"",
            "\"a\\\r\nb\"",
            "\"\\q\"",
            "[==[ ]=] ]==]",
            "[==[ ]==]",
            "[=[]]=]",
            "[[a[[b]]",
            "[=[a[[b]=]",
            "[[\r\n]]",
        ]
        .map(str::to_owned),
    );
    assert_agrees(&candidates, &LuaVersion::ALL, is_string_literal_in, |text| {
        (text.to_owned(), vec![TokenKind::StringLiteral(text.into())])
    });
}

#[test]
fn the_interpolated_text_predicate_agrees_with_the_lexer() {
    let mut candidates = texts(&[""], b"\\{}`nuxz0259a\n\r\0 ", 4);
    candidates.extend(["\\u{10FFFF}", "\\u{110000}", "\\x4g", "\\z \n x"].map(str::to_owned));
    let versions: Vec<LuaVersion> =
        LuaVersion::ALL.into_iter().filter(|version| version.has_interpolated_strings()).collect();
    assert_agrees(
        &candidates,
        &versions,
        |text, _| is_interpolated_text(text),
        |text| {
            let segments = vec![
                TokenKind::InterpolatedStringBegin("".into()),
                TokenKind::InterpolatedStringEnd(text.into()),
            ];
            (format!("`{text}`"), segments)
        },
    );
}
