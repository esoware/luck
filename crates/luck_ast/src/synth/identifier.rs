//! Identifier validity per dialect, for names that come from outside the
//! AST, such as bytecode debug info.

use luck_token::{CompactString, LuaVersion, TokenKind};

/// Whether `name` lexes as an identifier in at least one supported dialect.
///
/// That means an ASCII identifier that isn't one of the 21 keywords every
/// dialect reserves. `goto`, `global`, and the Luau context-sensitive words
/// (`type`, `continue`, `export`) pass. For a single-dialect check, use
/// [`is_valid_identifier_in`].
#[must_use]
pub fn is_valid_identifier(name: &str) -> bool {
    TokenKind::is_identifier_shaped(name)
        && LuaVersion::ALL.iter().any(|&version| TokenKind::keyword(name, version).is_none())
}

/// Whether `name` lexes as an identifier under `version`.
///
/// `goto` is reserved on Lua 5.2-5.5 but is an identifier on 5.1 and Luau;
/// `global` is reserved only on 5.5.
#[must_use]
pub fn is_valid_identifier_in(name: &str, version: LuaVersion) -> bool {
    TokenKind::is_identifier_shaped(name) && TokenKind::keyword(name, version).is_none()
}

/// Deterministic valid-identifier form of `name`, safe in every supported
/// dialect.
///
/// Each invalid byte becomes `_`, and a `_` prefix goes on when the result
/// would be empty, start with a digit, or be reserved in any dialect
/// (`goto` and `global` included). Names already valid everywhere come back
/// unchanged, so the function is idempotent. Distinct inputs can collide
/// (`"a b"` and `"a-b"` both become `a_b`), so deduplication is up to the
/// caller. For a single-dialect form, use [`sanitize_identifier_in`].
#[must_use]
pub fn sanitize_identifier(name: &str) -> CompactString {
    sanitize_with(name, |candidate| {
        LuaVersion::ALL.iter().any(|&version| TokenKind::keyword(candidate, version).is_some())
    })
}

/// Deterministic form of `name` that's a valid identifier under `version`.
///
/// Uses the same scheme and collision caveat as [`sanitize_identifier`].
/// Words the dialect treats as identifiers pass through (`goto` on Luau).
#[must_use]
pub fn sanitize_identifier_in(name: &str, version: LuaVersion) -> CompactString {
    sanitize_with(name, |candidate| TokenKind::keyword(candidate, version).is_some())
}

fn sanitize_with(name: &str, is_reserved: impl Fn(&str) -> bool) -> CompactString {
    let mut sanitized = String::with_capacity(name.len().saturating_add(1));
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'_' {
            sanitized.push(char::from(byte));
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
    CompactString::from(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;

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
            assert_eq!(sanitize_identifier(name).as_str(), name);
        }
        for name in ["", "1st", "a b", "end", "goto", "\u{dc}nicode"] {
            let once = sanitize_identifier(name);
            assert_eq!(sanitize_identifier(&once), once);
        }
    }

    #[test]
    fn sanitize_forms() {
        assert_eq!(sanitize_identifier("").as_str(), "_");
        assert_eq!(sanitize_identifier("1st").as_str(), "_1st");
        assert_eq!(sanitize_identifier("a b").as_str(), "a_b");
        assert_eq!(sanitize_identifier("end").as_str(), "_end");
        assert_eq!(sanitize_identifier("goto").as_str(), "_goto");
    }

    #[test]
    fn sanitize_respects_pinned_version() {
        assert_eq!(sanitize_identifier_in("goto", LuaVersion::Luau).as_str(), "goto");
        assert_eq!(sanitize_identifier_in("goto", LuaVersion::Lua54).as_str(), "_goto");
        assert_eq!(sanitize_identifier_in("global", LuaVersion::Lua54).as_str(), "global");
        assert_eq!(sanitize_identifier_in("global", LuaVersion::Lua55).as_str(), "_global");

        // The sanitized form feeds identifier() without tripping its assert.
        let synth = Synth::new().with_version(LuaVersion::Luau);
        let token = synth.identifier(&sanitize_identifier_in("goto", LuaVersion::Luau));
        assert!(matches!(token.kind, TokenKind::Identifier(_)));
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
