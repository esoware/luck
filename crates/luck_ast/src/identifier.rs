use std::borrow::Cow;

use luck_token::TokenKind;

/// Whether `name` lexes as an identifier: an ASCII identifier that isn't one
/// of the 21 keywords. A contextual keyword such as `type` or `continue`
/// passes.
pub fn is_valid_identifier(name: &str) -> bool {
    TokenKind::is_identifier_shaped(name) && !TokenKind::is_reserved(name)
}

/// Deterministic valid-identifier form of `name`.
///
/// Each invalid byte becomes `_`, and a `_` prefix goes on when the result
/// would be empty, start with a digit, or be a keyword. A valid name comes
/// back unchanged and borrowed, so the function is idempotent. Distinct
/// inputs can collide (`"a b"` and `"a-b"` both become `a_b`), so
/// deduplication is up to the caller.
pub fn sanitize_identifier(name: &str) -> Cow<'_, str> {
    if is_valid_identifier(name) {
        return Cow::Borrowed(name);
    }
    let mut sanitized = String::with_capacity(name.len() + 1);
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' {
            sanitized.push(char::from(b));
        } else {
            sanitized.push('_');
        }
    }
    let should_prefix = sanitized
        .as_bytes()
        .first()
        .is_none_or(|first| first.is_ascii_digit() || TokenKind::is_reserved(&sanitized));
    if should_prefix {
        sanitized.insert(0, '_');
    }
    Cow::Owned(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::Fixture;

    #[test]
    fn a_name_is_valid_where_it_reads_as_an_identifier() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("snake_case2"));
        // Contextual keywords are names.
        assert!(is_valid_identifier("type"));
        assert!(is_valid_identifier("continue"));
        assert!(is_valid_identifier("goto"));
        assert!(is_valid_identifier("global"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1st"));
        assert!(!is_valid_identifier("has space"));
        assert!(!is_valid_identifier("end"));
        assert!(!is_valid_identifier("\u{dc}nicode"));
    }

    #[test]
    fn sanitizing_yields_a_valid_identifier() {
        let hostile_names = [
            "",
            "1st",
            "has space",
            "a-b.c",
            "\tx\n",
            "end",
            "and",
            "\u{dc}nicode",
            "\u{1f600}",
            "_",
            "(*temporary)",
        ];
        for name in hostile_names {
            let sanitized = sanitize_identifier(name);
            assert!(is_valid_identifier(&sanitized), "{name:?} -> {sanitized:?} not valid");
        }
    }

    #[test]
    fn sanitizing_borrows_a_valid_name_and_is_idempotent() {
        for name in ["x", "_private", "snake_case2", "type", "continue", "goto"] {
            assert!(matches!(sanitize_identifier(name), Cow::Borrowed(kept) if kept == name));
        }
        for name in ["", "1st", "a b", "end", "\u{dc}nicode"] {
            let once = sanitize_identifier(name);
            assert_eq!(sanitize_identifier(&once), once);
        }
    }

    #[test]
    fn sanitizing_underscores_bad_bytes_and_prefixes_a_bad_start() {
        assert_eq!(sanitize_identifier(""), "_");
        assert_eq!(sanitize_identifier("1st"), "_1st");
        assert_eq!(sanitize_identifier("a b"), "a_b");
        assert_eq!(sanitize_identifier("end"), "_end");
        assert_eq!(sanitize_identifier("goto"), "goto");
    }

    #[test]
    fn a_sanitized_name_feeds_synth_name() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(synth.name(&sanitize_identifier("goto")).text, "goto");
        assert_eq!(synth.name(&sanitize_identifier("end")).text, "_end");
    }
}
