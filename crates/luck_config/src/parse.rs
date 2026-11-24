use json5::char::{is_json5_identifier, is_json5_line_terminator};
use luck_diagnostics::Diagnostic;
use luck_token::Span;

use crate::config::LuckConfig;

pub(crate) fn parse(text: &str) -> Result<LuckConfig, Diagnostic> {
    json5::from_str(text).map_err(|e| {
        let message = e.to_string();
        let (problem, span) = match e.position() {
            Some(position) => {
                let problem = message.strip_suffix(&format!(" at {position}")).unwrap_or(&message);
                (problem, Some(token_at(text, offset_of(text, position))))
            }
            None => (message.as_str(), None),
        };
        Diagnostic::e014(span, problem)
    })
}

/// The byte offset of `position`, which counts lines and characters the way
/// JSON5 does.
fn offset_of(text: &str, position: json5::Position) -> usize {
    let mut chars = text.char_indices().peekable();
    let mut line = 0;
    while line < position.line {
        let Some((_, c)) = chars.next() else {
            return text.len();
        };
        if is_json5_line_terminator(c) {
            if c == '\r' {
                chars.next_if(|&(_, next)| next == '\n');
            }
            line += 1;
        }
    }
    chars.nth(position.column).map_or(text.len(), |(offset, _)| offset)
}

/// The token that starts at `start`: a quoted string, or a run of the
/// characters a key, a number, or a literal is made of, so an error
/// underlines the word at fault.
fn token_at(text: &str, start: usize) -> Span {
    let rest = &text[start..];
    let mut chars = rest.char_indices();
    let length = match chars.next() {
        None => 0,
        Some((_, quote @ ('"' | '\''))) => {
            let mut is_escaped = false;
            chars
                .find(|&(_, c)| {
                    let is_closing = c == quote && !is_escaped;
                    is_escaped = c == '\\' && !is_escaped;
                    is_closing
                })
                .map_or(rest.len(), |(index, _)| index + 1)
        }
        Some((_, first)) if is_word_char(first) => {
            chars.find(|&(_, c)| !is_word_char(c)).map_or(rest.len(), |(index, _)| index)
        }
        Some((_, first)) => first.len_utf8(),
    };
    Span::from_indices(start, start + length)
}

fn is_word_char(c: char) -> bool {
    is_json5_identifier(c) || matches!(c, '+' | '-' | '.')
}

/// Where `name` is written in `text` as a string, or the start when it's
/// written another way, such as through an escape.
pub(crate) fn string_span(text: &str, name: &str) -> Span {
    ['"', '\'']
        .iter()
        .find_map(|quote| text.find(&format!("{quote}{name}{quote}")))
        .map_or_else(Span::default, |start| Span::from_indices(start, start + name.len() + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_json5_at_the_place_it_stops() {
        let diag = parse(
            r#"{
  lua: "lua54" "luau"
}"#,
        )
        .unwrap_err();
        assert_eq!(diag.code, "E014");
        assert_eq!(diag.message, "invalid config: expected comma");
        assert_eq!(diag.span, Some(Span::new(17, 23)));
    }

    #[test]
    fn places_an_unknown_key_on_the_key() {
        let text = "{\r\n  \"lua\": \"lua54\",\r\n  unknown_key: true\r\n}";
        let diag = parse(text).unwrap_err();
        assert!(
            diag.message.starts_with("invalid config: unknown field `unknown_key`"),
            "{diag:?}"
        );
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"unknown_key"[..]));
    }

    #[test]
    fn places_a_bad_value_on_the_value() {
        let text = "{ \"lua\": 'lua99' }";
        let diag = parse(text).unwrap_err();
        assert!(diag.message.contains("unknown variant `lua99`"), "{diag:?}");
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"'lua99'"[..]));
    }

    #[test]
    fn places_an_unknown_rule_on_its_name() {
        let text = "{ lint: { rules: { unsued_variable: 'off' } } }";
        let diag = parse(text).unwrap_err();
        assert_eq!(
            diag.message,
            "invalid config: unknown lint rule `unsued_variable`, did you mean `unused_variable`?"
        );
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"unsued_variable"[..]));
    }

    #[test]
    fn parses_json5_comments_unquoted_keys_and_trailing_commas() {
        let config = parse(
            r#"{
            // this is a comment
            lua: "lua54",
            minify: true, // trailing comma
        }"#,
        )
        .expect("JSON5 features should parse");
        assert_eq!(config.minify, Some(true));
    }

    #[test]
    fn offset_of_counts_json5_line_terminators_and_characters() {
        let text = "a\r\nb\u{2028}\u{e9}cd";
        let at = |line, column| offset_of(text, json5::Position { line, column });
        assert_eq!(at(1, 0), 3);
        assert_eq!(at(2, 1), 9);
        assert_eq!(at(5, 0), text.len());
    }
}
