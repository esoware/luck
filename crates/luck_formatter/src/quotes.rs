use alloc::borrow::Cow;

use crate::QuoteStyle;

pub(crate) fn normalize_quote(literal: &str, style: QuoteStyle) -> Cow<'_, str> {
    if literal.starts_with('[') {
        return Cow::Borrowed(literal);
    }

    let mut delimited = literal.chars();
    let Some(current_quote) = delimited.next() else {
        return Cow::Borrowed(literal);
    };
    delimited.next_back();
    let content = delimited.as_str();
    let target_quote = resolve_target_quote(style, content);

    if current_quote == target_quote {
        return Cow::Borrowed(literal);
    }

    let mut requoted = String::with_capacity(literal.len());
    requoted.push(target_quote);

    // Walk chars, not bytes, to keep UTF-8 intact. Each escape consumes both
    // of its chars, or a `\\` followed by a quote would read as an escaped
    // quote.
    let mut chars = content.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.next() {
                // Unescape old delimiter: \' -> ' (or \" -> ")
                Some(escaped) if escaped == current_quote => requoted.push(current_quote),
                // Any other escape, including an already-escaped target
                // delimiter, is copied as a pair.
                Some(escaped) => {
                    requoted.push('\\');
                    requoted.push(escaped);
                }
                None => requoted.push('\\'),
            }
        } else if character == target_quote {
            requoted.push('\\');
            requoted.push(target_quote);
        } else {
            requoted.push(character);
        }
    }

    requoted.push(target_quote);

    let original_escapes = content.matches('\\').count();
    let requoted_escapes = requoted.matches('\\').count();
    if requoted_escapes > original_escapes {
        return Cow::Borrowed(literal);
    }

    Cow::Owned(requoted)
}

/// Picks the delimiter for `style`.
///
/// Auto modes count each quote kind in the content, escaped or not, since
/// every quote matching the delimiter needs an escape. They leave the
/// preferred quote only when the other needs strictly fewer escapes. The
/// counts don't depend on the current delimiter, so the choice is stable
/// across reformats.
fn resolve_target_quote(style: QuoteStyle, content: &str) -> char {
    let (preferred, other) = match style {
        QuoteStyle::Double | QuoteStyle::AutoPreferDouble => ('"', '\''),
        QuoteStyle::Single | QuoteStyle::AutoPreferSingle => ('\'', '"'),
    };
    match style {
        QuoteStyle::Double | QuoteStyle::Single => preferred,
        QuoteStyle::AutoPreferDouble | QuoteStyle::AutoPreferSingle => {
            let preferred_escapes = content.matches(preferred).count();
            let other_escapes = content.matches(other).count();
            if other_escapes < preferred_escapes { other } else { preferred }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_to_double() {
        assert_eq!(normalize_quote("'hello'", QuoteStyle::Double), "\"hello\"");
    }

    #[test]
    fn already_correct_style() {
        assert_eq!(normalize_quote("\"hello\"", QuoteStyle::Double), "\"hello\"");
    }

    #[test]
    fn keep_current_when_target_in_content() {
        // Switching to double would need escapes, so single stays.
        assert_eq!(normalize_quote("'say \"hi\"'", QuoteStyle::Double), "'say \"hi\"'");
    }

    #[test]
    fn swap_when_current_in_content_but_target_not() {
        // The escaped single quote loses its backslash under double quotes.
        assert_eq!(normalize_quote("'it\\'s'", QuoteStyle::Double), "\"it's\"");
    }

    #[test]
    fn long_bracket_unchanged() {
        assert_eq!(normalize_quote("[[hello]]", QuoteStyle::Double), "[[hello]]");
        assert_eq!(normalize_quote("[=[hello]=]", QuoteStyle::Single), "[=[hello]=]");
    }

    #[test]
    fn normalize_to_single() {
        assert_eq!(normalize_quote("\"hello\"", QuoteStyle::Single), "'hello'");
    }

    #[test]
    fn empty_string() {
        assert_eq!(normalize_quote("\"\"", QuoteStyle::Single), "''");
    }

    #[test]
    fn unescape_old_delimiter_single_to_double() {
        assert_eq!(normalize_quote("'it\\'s here'", QuoteStyle::Double), "\"it's here\"");
    }

    #[test]
    fn unescape_old_delimiter_double_to_single() {
        assert_eq!(normalize_quote("\"say \\\"hi\\\"\"", QuoteStyle::Single), "'say \"hi\"'");
    }

    #[test]
    fn keep_original_when_both_quotes_in_content() {
        // Content has both quote kinds, and swapping would add escapes.
        assert_eq!(
            normalize_quote("'has \"both\" and it\\'s'", QuoteStyle::Double),
            "'has \"both\" and it\\'s'"
        );
    }

    #[test]
    fn simple_swap_no_escapes() {
        assert_eq!(normalize_quote("\"simple\"", QuoteStyle::Single), "'simple'");
    }

    #[test]
    fn auto_prefer_double_defaults_to_double() {
        assert_eq!(normalize_quote("'hello'", QuoteStyle::AutoPreferDouble), "\"hello\"");
        assert_eq!(normalize_quote("\"hello\"", QuoteStyle::AutoPreferDouble), "\"hello\"");
    }

    #[test]
    fn auto_prefer_double_switches_on_double_heavy_content() {
        assert_eq!(
            normalize_quote("\"say \\\"hi\\\"\"", QuoteStyle::AutoPreferDouble),
            "'say \"hi\"'"
        );
        // Already single stays single.
        assert_eq!(normalize_quote("'say \"hi\"'", QuoteStyle::AutoPreferDouble), "'say \"hi\"'");
    }

    #[test]
    fn auto_prefer_single_defaults_to_single() {
        assert_eq!(normalize_quote("\"hello\"", QuoteStyle::AutoPreferSingle), "'hello'");
    }

    #[test]
    fn auto_prefer_single_switches_on_single_heavy_content() {
        assert_eq!(normalize_quote("'it\\'s'", QuoteStyle::AutoPreferSingle), "\"it's\"");
    }

    #[test]
    fn auto_tie_keeps_preferred() {
        // One quote of each kind: no strict win, preferred quote stands.
        assert_eq!(normalize_quote("'it\\'s \"x'", QuoteStyle::AutoPreferDouble), "\"it's \\\"x\"");
        assert_eq!(normalize_quote("\"it's \\\"x\"", QuoteStyle::AutoPreferSingle), "'it\\'s \"x'");
    }

    #[test]
    fn auto_both_kinds_present_keeps_winner_with_escapes() {
        // Two doubles vs one single: single wins under either preference, so
        // the doubles go unescaped and the single gets escaped.
        assert_eq!(
            normalize_quote("\"it's \\\"x\\\"\"", QuoteStyle::AutoPreferDouble),
            "'it\\'s \"x\"'"
        );
        assert_eq!(
            normalize_quote("'it\\'s \"x\"'", QuoteStyle::AutoPreferDouble),
            "'it\\'s \"x\"'"
        );
    }

    #[test]
    fn auto_long_bracket_unchanged() {
        assert_eq!(
            normalize_quote("[[has \"lots\" of \"doubles\"]]", QuoteStyle::AutoPreferDouble),
            "[[has \"lots\" of \"doubles\"]]"
        );
    }

    #[test]
    fn auto_is_idempotent() {
        for literal in [
            "'hello'",
            "\"say \\\"hi\\\"\"",
            "'it\\'s \"x\"'",
            "\"mixed 'a' and \\\"b\\\" and \\\"c\\\"\"",
        ] {
            for style in [QuoteStyle::AutoPreferDouble, QuoteStyle::AutoPreferSingle] {
                let once = normalize_quote(literal, style);
                assert_eq!(normalize_quote(&once, style), once, "{literal} @ {style:?}");
            }
        }
    }
}
