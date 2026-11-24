use std::borrow::Cow;

use luck_token::{LuaVersion, TokenKind};

/// Whether `name` lexes as an identifier in at least one supported dialect.
///
/// That means an ASCII identifier that isn't one of the 21 keywords every
/// dialect reserves. `goto`, `global`, and the Luau context-sensitive words
/// (`type`, `continue`, `export`) pass. For a single-dialect check, use
/// [`is_valid_identifier_in`].
pub fn is_valid_identifier(name: &str) -> bool {
    TokenKind::is_identifier_shaped(name)
        && LuaVersion::ALL.iter().any(|&version| !TokenKind::is_reserved(name, version))
}

/// `goto` is reserved on Lua 5.2-5.5 but is an identifier on 5.1 and Luau;
/// `global` is refused only on 5.5, where some builds reserve it.
pub fn is_valid_identifier_in(name: &str, version: LuaVersion) -> bool {
    TokenKind::is_identifier_shaped(name) && !TokenKind::is_reserved(name, version)
}

/// Deterministic valid-identifier form of `name`, safe in every supported
/// dialect.
///
/// Each invalid byte becomes `_`, and a `_` prefix goes on when the result
/// would be empty, start with a digit, or be reserved in any dialect
/// (`goto` and `global` included). Names already valid everywhere come back
/// unchanged and borrowed, so the function is idempotent. Distinct inputs can
/// collide (`"a b"` and `"a-b"` both become `a_b`), so deduplication is up to
/// the caller. For a single-dialect form, use [`sanitize_identifier_in`].
pub fn sanitize_identifier(name: &str) -> Cow<'_, str> {
    sanitize_with(name, |candidate| {
        LuaVersion::ALL.iter().any(|&version| TokenKind::is_reserved(candidate, version))
    })
}

/// Deterministic form of `name` that's a valid identifier under `version`.
///
/// Uses the same scheme and collision caveat as [`sanitize_identifier`].
/// Words the dialect treats as identifiers pass through (`goto` on Luau).
pub fn sanitize_identifier_in(name: &str, version: LuaVersion) -> Cow<'_, str> {
    sanitize_with(name, |candidate| TokenKind::is_reserved(candidate, version))
}

fn sanitize_with(name: &str, is_reserved: impl Fn(&str) -> bool) -> Cow<'_, str> {
    if TokenKind::is_identifier_shaped(name) && !is_reserved(name) {
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
        .is_none_or(|first| first.is_ascii_digit() || is_reserved(&sanitized));
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
    fn identifier_validity() {
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("_private"));
        assert!(is_valid_identifier("snake_case2"));
        // Context-sensitive words are identifiers.
        assert!(is_valid_identifier("type"));
        assert!(is_valid_identifier("continue"));
        assert!(is_valid_identifier("goto"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1st"));
        assert!(!is_valid_identifier("has space"));
        assert!(!is_valid_identifier("end"));
        assert!(!is_valid_identifier("\u{dc}nicode"));
    }

    #[test]
    fn sanitize_always_yields_valid_identifiers() {
        let hostile_names = [
            "",
            "1st",
            "has space",
            "a-b.c",
            "\tx\n",
            "end",
            "and",
            "goto",
            "global",
            "\u{dc}nicode",
            "\u{1f600}",
            "_",
            "(*temporary)",
        ];
        for name in hostile_names {
            let sanitized = sanitize_identifier(name);
            assert!(
                is_valid_identifier(&sanitized),
                "{name:?} -> {sanitized:?} not valid cross-dialect"
            );
            for version in LuaVersion::ALL {
                assert!(
                    is_valid_identifier_in(&sanitized, version),
                    "{name:?} -> {sanitized:?} not valid in {version:?}"
                );
                let pinned = sanitize_identifier_in(name, version);
                assert!(
                    is_valid_identifier_in(&pinned, version),
                    "{name:?} -> {pinned:?} not valid in {version:?}"
                );
            }
        }
    }

    #[test]
    fn sanitize_keeps_valid_names_and_is_idempotent() {
        for name in ["x", "_private", "snake_case2", "type", "continue"] {
            assert!(matches!(sanitize_identifier(name), Cow::Borrowed(kept) if kept == name));
        }
        for name in ["", "1st", "a b", "end", "goto", "\u{dc}nicode"] {
            let once = sanitize_identifier(name);
            assert_eq!(sanitize_identifier(&once), once);
        }
    }

    #[test]
    fn sanitize_forms() {
        assert_eq!(sanitize_identifier(""), "_");
        assert_eq!(sanitize_identifier("1st"), "_1st");
        assert_eq!(sanitize_identifier("a b"), "a_b");
        assert_eq!(sanitize_identifier("end"), "_end");
        assert_eq!(sanitize_identifier("goto"), "_goto");
    }

    #[test]
    fn sanitize_respects_pinned_version() {
        assert_eq!(sanitize_identifier_in("goto", LuaVersion::Luau), "goto");
        assert_eq!(sanitize_identifier_in("goto", LuaVersion::Lua54), "_goto");
        assert_eq!(sanitize_identifier_in("global", LuaVersion::Lua54), "global");
        assert_eq!(sanitize_identifier_in("global", LuaVersion::Lua55), "_global");

        // The sanitized form feeds `name` without tripping its assert.
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        assert_eq!(synth.name(&sanitize_identifier_in("goto", LuaVersion::Luau)).text, "goto");
    }

    #[test]
    fn identifier_validity_per_version() {
        assert!(is_valid_identifier_in("goto", LuaVersion::Lua51));
        assert!(is_valid_identifier_in("goto", LuaVersion::Luau));
        assert!(!is_valid_identifier_in("goto", LuaVersion::Lua54));
        assert!(is_valid_identifier_in("global", LuaVersion::Lua54));
        assert!(!is_valid_identifier_in("global", LuaVersion::Lua55));
        assert!(!is_valid_identifier_in("end", LuaVersion::Luau));
        assert!(!is_valid_identifier_in("", LuaVersion::Luau));
    }
}
