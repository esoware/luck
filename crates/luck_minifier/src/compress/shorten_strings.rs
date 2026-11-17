//! Respelling a string literal shorter, through its decoded bytes, which
//! keeps every escape, UTF-8 sequence, and arbitrary byte exact.

use luck_allocator::Allocator;
use luck_ast::Literal;
use luck_token::{LuaVersion, decode_string, encode_string, encodes_to_at_least};

use super::literal_cache::LiteralCache;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the cache is one type with its respellings split by flag"
)]
impl LiteralCache {
    /// A string literal, as written, quotes or long brackets included.
    pub(super) fn shorten_string<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        version: LuaVersion,
        literal: &mut Literal<'ast>,
    ) -> bool {
        // A quoted string without escapes encodes to at least its own length.
        if !literal.text.starts_with('[') && !literal.text.contains('\\') {
            return false;
        }
        self.shorten(allocator, literal, |text, respelled| {
            let Some(bytes) = decode_string(text, version) else {
                return false;
            };
            if encodes_to_at_least(&bytes, text.len()) {
                return false;
            }
            encode_string(&bytes, respelled);
            true
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const STRINGS: TransformConfig = TransformConfig { shorten_strings: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &STRINGS)
    }

    #[test]
    fn respells_a_string_shorter_and_exact() {
        for (source, expected) in [
            ("local x = [[hello]]", "local x=\"hello\""),
            ("local x = [=[hello]=]", "local x=\"hello\""),
            ("local x = [[hello\nworld]]", "local x=\"hello\\nworld\""),
            ("local x = \"\\097\\098\\099\"", "local x=\"abc\""),
            ("local x = \"\\000\"", "local x=\"\\0\""),
            ("local x = \"\u{e9}\\097\\098\\099\"", "local x=\"\u{e9}abc\""),
            ("local t = {[[hello]], [[world]]}", "local t={\"hello\",\"world\"}"),
        ] {
            assert_eq!(apply(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_a_string_no_shorter_spelling_beats() {
        for source in [
            "local x=\"hello\"",
            // The re-encoded forms are the same length.
            "local x='hello'",
            "local x=[[say \"hi\"]]",
            // `\\097` is a literal backslash followed by "097", not the byte 97.
            "local x=\"\\\\097\"",
        ] {
            assert_eq!(apply(source), source);
        }
    }
}
