use rustc_hash::FxHashMap;

/// This reads Luau's own grammar: bytes rather than UTF-8, nesting on the heap,
/// and a repeated key merging into the earlier ones.
pub(crate) fn parse_aliases(file_bytes: &[u8]) -> Option<FxHashMap<String, String>> {
    let mut reader =
        LuaurcReader { rest: file_bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(file_bytes) };
    let mut aliases = FxHashMap::default();
    let mut keys: Vec<&[u8]> = Vec::new();
    let mut is_in_array = false;
    if !reader.eat(b'{') {
        return None;
    }
    loop {
        if is_in_array {
            if reader.eat(b']') {
                is_in_array = false;
                keys.pop();
                reader.expect_separator(b'}')?;
            } else {
                reader.string()?;
                reader.expect_separator(b']')?;
            }
        } else if reader.eat(b'}') {
            if keys.pop().is_none() {
                return reader.peek().is_none().then_some(aliases);
            }
            reader.expect_separator(b'}')?;
        } else {
            keys.push(reader.string()?);
            if !reader.eat(b':') {
                return None;
            }
            if reader.eat(b'[') {
                is_in_array = true;
            } else if reader.eat(b'{') {
                // Its keys come next, under this one.
            } else {
                // Luau hands a boolean on as its text, so one in `aliases` is
                // just an odd target.
                let (value, is_boolean) = if reader.eat_word(b"true") {
                    (b"true".as_slice(), true)
                } else if reader.eat_word(b"false") {
                    (b"false".as_slice(), true)
                } else {
                    (reader.string()?, false)
                };
                match keys.as_slice() {
                    [b"aliases", name] if !is_valid_alias_name(name) => return None,
                    [b"aliases", name] => {
                        let lowercase_name = String::from_utf8_lossy(name).to_ascii_lowercase();
                        // `parseAlias` tests the key as written against its
                        // lowercased entries, so only a later name with an
                        // uppercase letter replaces an equal one.
                        if lowercase_name.as_bytes() != *name
                            || !aliases.contains_key(&lowercase_name)
                        {
                            aliases.insert(
                                lowercase_name,
                                String::from_utf8_lossy(value).into_owned(),
                            );
                        }
                    }
                    [b"languageMode"] if is_boolean => return None,
                    _ => {}
                }
                keys.pop();
                reader.expect_separator(b'}')?;
            }
        }
    }
}

/// Mirrors Luau's `isValidAlias`, which fails the whole file on a bad name.
fn is_valid_alias_name(name: &[u8]) -> bool {
    let unprefixed = name.strip_prefix(b"@").unwrap_or(name);
    !name.is_empty()
        && !matches!(name, b"." | b"..")
        && unprefixed.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

struct LuaurcReader<'contents> {
    rest: &'contents [u8],
}

impl<'contents> LuaurcReader<'contents> {
    /// A long comment that never closes yields its `-`, which no caller accepts.
    fn peek(&mut self) -> Option<u8> {
        loop {
            match self.rest {
                [b' ' | b'\t' | b'\r' | b'\n' | b'\x0b' | b'\x0c', tail @ ..] => {
                    self.rest = tail;
                }
                [b'/', b'/', ..] => self.skip_line(),
                [b'-', b'-', after_dashes @ ..] => {
                    let level = long_bracket_level(after_dashes);
                    match level.map(|found_level| after_long_bracket(after_dashes, found_level)) {
                        Some(Some(after_comment)) => self.rest = after_comment,
                        Some(None) => return Some(b'-'),
                        None => self.skip_line(),
                    }
                }
                [byte, ..] => return Some(*byte),
                [] => return None,
            }
        }
    }

    fn skip_line(&mut self) {
        let line_end =
            self.rest.iter().position(|b| matches!(b, b'\r' | b'\n')).unwrap_or(self.rest.len());
        self.rest = &self.rest[line_end..];
    }

    fn eat(&mut self, byte: u8) -> bool {
        let is_next = self.peek() == Some(byte);
        if is_next {
            self.rest = &self.rest[1..];
        }
        is_next
    }

    fn eat_word(&mut self, word: &[u8]) -> bool {
        self.peek();
        let after_word = self.rest.strip_prefix(word);
        if let Some(tail) = after_word {
            self.rest = tail;
        }
        after_word.is_some()
    }

    /// A value's trailing `,`, or the `closer` left for the next read.
    fn expect_separator(&mut self, closer: u8) -> Option<()> {
        (self.eat(b',') || self.peek() == Some(closer)).then_some(())
    }

    /// Luau keeps escapes as written, so this only needs them to find the closing
    /// quote.
    fn string(&mut self) -> Option<&'contents [u8]> {
        let quote = self.peek().filter(|b| matches!(b, b'"' | b'\''))?;
        let body = &self.rest[1..];
        let mut remaining = body;
        loop {
            match remaining {
                [] | [b'\n' | b'\r' | 0, ..] => return None,
                [b'\\', b'\r', b'\n', tail @ ..] | [b'\\', _, tail @ ..] => remaining = tail,
                [byte, tail @ ..] if *byte == quote => {
                    self.rest = tail;
                    return Some(&body[..body.len() - tail.len() - 1]);
                }
                [_, tail @ ..] => remaining = tail,
            }
        }
    }
}

fn long_bracket_level(text: &[u8]) -> Option<usize> {
    let after_opening = text.strip_prefix(b"[")?;
    let level = after_opening.iter().take_while(|b| **b == b'=').count();
    (after_opening.get(level) == Some(&b'[')).then_some(level)
}

fn after_long_bracket(text: &[u8], level: usize) -> Option<&[u8]> {
    let mut remaining = &text[level + 2..];
    loop {
        let closer = remaining.iter().position(|b| *b == b']')?;
        let after_closer = &remaining[closer + 1..];
        let equals_count = after_closer.iter().take_while(|b| **b == b'=').count();
        if equals_count == level && after_closer.get(level) == Some(&b']') {
            return Some(&after_closer[level + 1..]);
        }
        remaining = after_closer;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luaurc_accepts_json5_comments_and_trailing_commas() {
        let aliases = parse_aliases(
            br#"{
            // This is a comment
            "languageMode": "strict",
            "aliases": {
                "utils": "../shared/utils",
                "components": "./components",
            },  // trailing comma
        }"#,
        )
        .expect("parses");
        assert_eq!(aliases.get("utils").map(String::as_str), Some("../shared/utils"));
        assert_eq!(aliases.get("components").map(String::as_str), Some("./components"));
    }

    #[test]
    fn luaurc_accepts_every_comment_form_the_luau_lexer_does() {
        let aliases = parse_aliases(
            b"-- line\r{ --[[ block\n}]] // line\n --[==[ ]] ]==] \"aliases\": {\"a\": \"./a\"} } --x",
        )
        .expect("parses");
        assert_eq!(aliases.get("a").map(String::as_str), Some("./a"));
    }

    #[test]
    fn luaurc_with_an_unterminated_long_comment_is_malformed() {
        let contents = b"\
--[[ never closed
{}";
        assert_eq!(parse_aliases(contents), None);
    }

    #[test]
    fn luaurc_string_broken_by_a_carriage_return_is_malformed() {
        assert_eq!(parse_aliases(b"{\"aliases\": {\"a\": \"./a\r\"}}"), None);
    }

    #[test]
    fn luaurc_boolean_alias_target_is_kept_as_its_text() {
        let aliases = parse_aliases(br#"{"aliases": {"a": true}}"#).expect("parses");
        assert_eq!(aliases.get("a").map(String::as_str), Some("true"));
    }

    #[test]
    fn luaurc_without_aliases_declares_none() {
        assert_eq!(parse_aliases(b"{}"), Some(FxHashMap::default()));
    }

    #[test]
    fn luaurc_that_is_not_json_is_malformed() {
        assert_eq!(parse_aliases(b"not valid json {{{"), None);
    }
}
