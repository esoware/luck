use crate::QuoteStyle;

pub(crate) fn normalize_quote(literal: &str, style: QuoteStyle) -> String {
    if literal.starts_with('[') {
        return literal.to_string();
    }

    let current_quote = literal.as_bytes()[0] as char;
    let content = &literal[1..literal.len() - 1];
    let target_quote = resolve_target_quote(style, content);

    if current_quote == target_quote {
        return literal.to_string();
    }

    // Transform escape sequences for the delimiter swap:
    // 1. Unescape old delimiter: \' -> ' (or \" -> ")
    // 2. Escape new delimiter: " -> \" (or ' -> \')
    let mut result = String::with_capacity(content.len() + 2);
    result.push(target_quote);

    // Char-wise walk: byte-wise reinterpretation corrupts UTF-8, and every
    // escape must consume BOTH chars or `\\` followed by a quote merges
    // into a bogus escape.
    let mut chars = content.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                // Unescape old delimiter: \' -> ' (or \" -> ")
                Some(escaped) if escaped == current_quote => result.push(current_quote),
                // Any other escape (incl. already-escaped target delimiter,
                // \\, \n, \ddd): copy the pair untouched.
                Some(escaped) => {
                    result.push('\\');
                    result.push(escaped);
                }
                None => result.push('\\'),
            }
        } else if ch == target_quote {
            // Unescaped target delimiter in content - needs escaping
            result.push('\\');
            result.push(target_quote);
        } else {
            result.push(ch);
        }
    }

    result.push(target_quote);

    // If the result has more escapes than the original, keep the original
    let original_escapes = content.matches('\\').count();
    let new_content = &result[1..result.len() - 1];
    let result_escapes = new_content.matches('\\').count();
    if result_escapes > original_escapes {
        return literal.to_string();
    }

    result
}

/// Pick the delimiter for `style`. Auto modes count the quote characters of
/// each kind in the content (escaped or not - every one of the delimiter's
/// kind must be escaped in the output) and switch away from the preferred
/// quote only when the other needs strictly fewer escapes; ties keep the
/// preferred quote. The counts are delimiter-independent, so the choice is
/// stable across reformats.
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
            if other_escapes < preferred_escapes {
                other
            } else {
                preferred
            }
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
        assert_eq!(
            normalize_quote("\"hello\"", QuoteStyle::Double),
            "\"hello\""
        );
    }

    #[test]
    fn keep_current_when_target_in_content() {
        // Content has double quotes, so keep single to avoid needing escapes
        assert_eq!(
            normalize_quote("'say \"hi\"'", QuoteStyle::Double),
            "'say \"hi\"'"
        );
    }

    #[test]
    fn swap_when_current_in_content_but_target_not() {
        // Content has escaped single quote - unescape when swapping to double
        assert_eq!(normalize_quote("'it\\'s'", QuoteStyle::Double), "\"it's\"");
    }

    #[test]
    fn long_bracket_unchanged() {
        assert_eq!(
            normalize_quote("[[hello]]", QuoteStyle::Double),
            "[[hello]]"
        );
        assert_eq!(
            normalize_quote("[=[hello]=]", QuoteStyle::Single),
            "[=[hello]=]"
        );
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
        assert_eq!(
            normalize_quote("'it\\'s here'", QuoteStyle::Double),
            "\"it's here\""
        );
    }

    #[test]
    fn unescape_old_delimiter_double_to_single() {
        assert_eq!(
            normalize_quote("\"say \\\"hi\\\"\"", QuoteStyle::Single),
            "'say \"hi\"'"
        );
    }

    #[test]
    fn keep_original_when_both_quotes_in_content() {
        // Content has both quote types - swapping would add more escapes
        assert_eq!(
            normalize_quote("'has \"both\" and it\\'s'", QuoteStyle::Double),
            "'has \"both\" and it\\'s'"
        );
    }

    #[test]
    fn simple_swap_no_escapes() {
        assert_eq!(
            normalize_quote("\"simple\"", QuoteStyle::Single),
            "'simple'"
        );
    }

    #[test]
    fn auto_prefer_double_defaults_to_double() {
        assert_eq!(
            normalize_quote("'hello'", QuoteStyle::AutoPreferDouble),
            "\"hello\""
        );
        assert_eq!(
            normalize_quote("\"hello\"", QuoteStyle::AutoPreferDouble),
            "\"hello\""
        );
    }

    #[test]
    fn auto_prefer_double_switches_on_double_heavy_content() {
        assert_eq!(
            normalize_quote("\"say \\\"hi\\\"\"", QuoteStyle::AutoPreferDouble),
            "'say \"hi\"'"
        );
        // Already single stays single.
        assert_eq!(
            normalize_quote("'say \"hi\"'", QuoteStyle::AutoPreferDouble),
            "'say \"hi\"'"
        );
    }

    #[test]
    fn auto_prefer_single_defaults_to_single() {
        assert_eq!(
            normalize_quote("\"hello\"", QuoteStyle::AutoPreferSingle),
            "'hello'"
        );
    }

    #[test]
    fn auto_prefer_single_switches_on_single_heavy_content() {
        assert_eq!(
            normalize_quote("'it\\'s'", QuoteStyle::AutoPreferSingle),
            "\"it's\""
        );
    }

    #[test]
    fn auto_tie_keeps_preferred() {
        // One quote of each kind: no strict win, preferred quote stands.
        assert_eq!(
            normalize_quote("'it\\'s \"x'", QuoteStyle::AutoPreferDouble),
            "\"it's \\\"x\""
        );
        assert_eq!(
            normalize_quote("\"it's \\\"x\"", QuoteStyle::AutoPreferSingle),
            "'it\\'s \"x'"
        );
    }

    #[test]
    fn auto_both_kinds_present_keeps_winner_with_escapes() {
        // Two doubles vs one single: single wins under either preference,
        // the double quotes ride along unescaped and the single is escaped.
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
            normalize_quote(
                "[[has \"lots\" of \"doubles\"]]",
                QuoteStyle::AutoPreferDouble
            ),
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
