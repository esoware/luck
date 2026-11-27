use std::iter;

use json5::char::{is_json5_identifier, is_json5_line_terminator, is_json5_whitespace};
use luck_diagnostics::Diagnostic;
use luck_token::Span;

use crate::config::LuckConfig;

/// How deep arrays and objects may nest. json5 reads each level with a
/// recursive call and no limit of its own, so a deeper file would overflow
/// the stack of whichever thread loads it.
const MAX_NESTING_DEPTH: usize = 128;

pub(crate) fn parse(text: &str) -> Result<LuckConfig, Diagnostic> {
    let mut depth = 0_usize;
    for (start, token) in tokens(text) {
        match token {
            "[" | "{" => depth += 1,
            "]" | "}" => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth > MAX_NESTING_DEPTH {
            let problem =
                format!("arrays and objects nest more than {MAX_NESTING_DEPTH} levels deep");
            return Err(Diagnostic::e014(Some(Span::from_indices(start, start + 1)), &problem));
        }
    }
    json5::from_str(text).map_err(|e| {
        let message = e.to_string();
        let (problem, span) = match e.position() {
            Some(position) => {
                let problem = message.strip_suffix(&format!(" at {position}")).unwrap_or(&message);
                let start = offset_of(text, position);
                (problem, Some(Span::from_indices(start, start + token_length(&text[start..]))))
            }
            None => (message.as_str(), None),
        };
        Diagnostic::e014(span, problem)
    })
}

/// Each token of `text`, a JSON5 document, with its offset, past the
/// whitespace and comments between them.
fn tokens(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    iter::from_fn(move || {
        loop {
            let rest = &text[offset..];
            if let Some(comment) = rest.strip_prefix("//") {
                offset += 2 + comment.find(is_json5_line_terminator).unwrap_or(comment.len());
            } else if let Some(comment) = rest.strip_prefix("/*") {
                offset += 2 + comment.find("*/").map_or(comment.len(), |end| end + 2);
            } else {
                let c = rest.chars().next()?;
                if !is_json5_whitespace(c) {
                    let start = offset;
                    offset += token_length(rest);
                    return Some((start, &text[start..offset]));
                }
                offset += c.len_utf8();
            }
        }
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

/// The length of the token `rest` starts with: a quoted string, or a run of
/// the characters a key, a number, or a literal is made of, so an error
/// underlines the word at fault, or else one character.
fn token_length(rest: &str) -> usize {
    let mut chars = rest.char_indices();
    match chars.next() {
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
    }
}

fn is_word_char(c: char) -> bool {
    is_json5_identifier(c) || matches!(c, '+' | '-' | '.')
}

/// One step from a config's top level toward an array: into an object's key
/// or an array's element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step<'key> {
    Key(&'key str),
    Index(usize),
}

/// Where `name` is written as an entry of the array `path` leads to in
/// `text`, a config, or the start when it's written another way, such as
/// through an escape.
pub(crate) fn entry_span(text: &str, path: &[Step<'_>], name: &str) -> Span {
    let mut frames: Vec<Frame<'_>> = Vec::new();
    for (start, token) in tokens(text) {
        match token {
            "{" => frames.push(Frame::Object { key: None, is_at_key: true }),
            "[" => frames.push(Frame::Array { index: 0 }),
            "}" | "]" => {
                frames.pop();
            }
            "," => match frames.last_mut() {
                Some(Frame::Object { is_at_key, .. }) => *is_at_key = true,
                Some(Frame::Array { index }) => *index += 1,
                None => {}
            },
            ":" => {
                if let Some(Frame::Object { is_at_key, .. }) = frames.last_mut() {
                    *is_at_key = false;
                }
            }
            _ => {
                if let Some(Frame::Object { key, is_at_key: true }) = frames.last_mut() {
                    *key = Some(string_content(token).unwrap_or(token));
                } else if string_content(token) == Some(name) && is_at(&frames, path) {
                    return Span::from_indices(start, start + token.len());
                }
            }
        }
    }
    Span::default()
}

/// An object or array a token is in, with the key or the index of the value
/// it's reading.
#[derive(Debug, Clone, Copy)]
enum Frame<'text> {
    Object { key: Option<&'text str>, is_at_key: bool },
    Array { index: usize },
}

/// Whether `frames`, from the top level in, end in the array `path` leads
/// to.
fn is_at(frames: &[Frame<'_>], path: &[Step<'_>]) -> bool {
    let Some((Frame::Array { .. }, outer)) = frames.split_last() else {
        return false;
    };
    outer.len() == path.len()
        && outer.iter().zip(path).all(|(frame, step)| match (frame, step) {
            (Frame::Object { key: Some(key), .. }, Step::Key(expected)) => key == expected,
            (Frame::Array { index }, Step::Index(expected)) => index == expected,
            (Frame::Object { .. } | Frame::Array { .. }, Step::Key(_) | Step::Index(_)) => false,
        })
}

/// What `token` holds between its quotes, or `None` when it isn't a whole
/// string.
fn string_content(token: &str) -> Option<&str> {
    let quote = token.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    token.strip_prefix(quote)?.strip_suffix(quote)
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn rejects_invalid_json5_at_the_place_it_stops() {
        let diag = parse(
            r#"{
  dynamic_require: "error" "warn"
}"#,
        )
        .unwrap_err();
        assert_eq!(diag.code, "E014");
        assert_eq!(diag.message, "invalid config: expected comma");
        assert_eq!(diag.span, Some(Span::new(29, 35)));
    }

    #[test]
    fn places_an_unknown_key_on_the_key() {
        let text = "{\r\n  \"dynamic_require\": \"warn\",\r\n  unknown_key: true\r\n}";
        let diag = parse(text).unwrap_err();
        assert!(
            diag.message.starts_with("invalid config: unknown field `unknown_key`"),
            "{diag:?}"
        );
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"unknown_key"[..]));
    }

    #[test]
    fn places_a_bad_value_on_the_value() {
        let text = "{ \"dynamic_require\": 'bogus' }";
        let diag = parse(text).unwrap_err();
        assert!(diag.message.contains("unknown variant `bogus`"), "{diag:?}");
        assert_eq!(diag.span.map(|span| span.text(text.as_bytes())), Some(&b"'bogus'"[..]));
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
            dynamic_require: "allow",
            minify: true, // trailing comma
        }"#,
        )
        .expect("JSON5 features should parse");
        assert_eq!(config.minify, Some(true));
    }

    /// A config whose arrays and objects nest `depth` levels deep, in a rule
    /// setting's third element, which takes any value.
    fn nested(depth: usize) -> String {
        let inner = depth - 4;
        format!(
            r#"{{ "lint": {{ "rules": {{ "cyclomatic_complexity": ["warn", {{ "max": 1 }}, {}{}] }} }} }}"#,
            "[".repeat(inner),
            "]".repeat(inner)
        )
    }

    #[test]
    fn rejects_nesting_past_the_limit_on_the_bracket_that_passes_it() {
        let text = nested(100_000);
        let deep = text.clone();
        let diag = thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || parse(&deep).unwrap_err())
            .expect("the thread starts")
            .join()
            .expect("the parse returns");
        assert_eq!(
            diag.message,
            "invalid config: arrays and objects nest more than 128 levels deep"
        );
        // Four levels lead to the run of brackets, so its 125th passes 128.
        let start = text.find("[[").expect("the run of brackets") + 124;
        assert_eq!(diag.span, Some(Span::from_indices(start, start + 1)));
    }

    #[test]
    fn leaves_nesting_at_the_limit_to_json5() {
        let diag = parse(&nested(128)).unwrap_err();
        assert!(diag.message.starts_with("invalid config: invalid length 3"), "{diag:?}");
    }

    #[test]
    fn counts_no_bracket_in_a_string_or_a_comment() {
        let brackets = "[".repeat(200);
        let text = format!("{{ // {brackets}\n /* {brackets} */ dynamic_require: '{brackets}' }}");
        let diag = parse(&text).unwrap_err();
        assert!(diag.message.contains("unknown variant"), "{diag:?}");
    }

    #[test]
    fn entry_span_finds_an_entry_of_the_top_level_key_alone() {
        let text = r#"{
  overrides: [{ extends: ['a.json'] }],
  "note": "extends", // 'a.json'
  extends: ['b.json', 'a.json'],
  "definitions": ["a.json"],
}"#;
        let extends = [Step::Key("extends")];
        let span = entry_span(text, &extends, "a.json");
        let start = text.find("'a.json'],").expect("the entry");
        assert_eq!(span, Span::from_indices(start, start + 8));
        assert_eq!(entry_span(text, &extends, "c.json"), Span::default());
    }

    #[test]
    fn entry_span_follows_a_path_into_an_override() {
        let text = r#"{
  "definitions": ["a.d.luau"],
  overrides: [
    { files: ["spec/**"], "definitions": ["a.d.luau"] },
    { files: ["a.d.luau"], definitions: ["b.d.luau", "a.d.luau"] },
  ],
}"#;
        let path = [Step::Key("overrides"), Step::Index(1), Step::Key("definitions")];
        let span = entry_span(text, &path, "a.d.luau");
        let start = text.rfind("\"a.d.luau\"").expect("the entry");
        assert_eq!(span, Span::from_indices(start, start + 10));
        let top = entry_span(text, &[Step::Key("definitions")], "a.d.luau");
        let first = text.find("\"a.d.luau\"").expect("the entry");
        assert_eq!(top, Span::from_indices(first, first + 10));
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
