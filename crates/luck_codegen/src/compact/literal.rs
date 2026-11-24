use luck_ast::{IntegerLiteral, NumberLiteral, StringLiteral};
use luck_token::{
    LuaVersion, StringSpelling, spell_integer, spell_number, spell_string,
    spell_string_shorter_than, write_interpolated_text,
};

use super::Printer;
use crate::separator::{DOT_DOT, LEFT_BRACKET, NUMBER, OTHER, WORD};

impl Printer {
    /// As written, or the shorter of that and the shortest spelling of its
    /// value, as written on a tie.
    pub(super) fn print_number(&mut self, literal: &NumberLiteral<'_>) {
        if let Some(raw) = literal.raw
            && (!self.is_shortest || is_shortest_numeral(raw.as_str(), self.version))
        {
            self.number(raw.as_str());
            return;
        }
        let spelling = spell_number(literal.value, self.version);
        self.number(shorter(literal.raw.as_deref(), spelling.as_str()));
    }

    /// Luau: an integer, as [`Self::print_number`] prints a number.
    pub(super) fn print_integer(&mut self, literal: &IntegerLiteral<'_>) {
        if let Some(raw) = literal.raw
            && !self.is_shortest
        {
            self.number(raw.as_str());
            return;
        }
        let spelling = spell_integer(literal.value);
        self.number(shorter(literal.raw.as_deref(), spelling.as_str()));
    }

    /// As [`Self::print_number`] prints a number. A quoted string with no
    /// escape is as short as its value can be spelled.
    pub(super) fn print_string(&mut self, literal: &StringLiteral<'_>) {
        let Some(raw) = literal.raw else {
            self.write_string_spelling(spell_string(&literal.value));
            return;
        };
        if !self.is_shortest || (matches!(raw.first(), Some(b'"' | b'\'')) && !raw.contains(&b'\\'))
        {
            self.string(raw.as_bytes());
            return;
        }
        match spell_string_shorter_than(&literal.value, raw.len()) {
            Some(spelling) => self.write_string_spelling(spelling),
            None => self.string(raw.as_bytes()),
        }
    }

    /// Luau: a piece of an interpolated string's text, which nothing merges
    /// with.
    pub(super) fn print_interpolated_text(&mut self, text: &StringLiteral<'_>) {
        if let Some(raw) = text.raw
            && (!self.is_shortest || !raw.contains(&b'\\'))
        {
            self.output.print_bytes(raw.as_bytes());
            return;
        }
        let start = self.output.len();
        self.output.print_with(|output| write_interpolated_text(&text.value, output));
        if let Some(raw) = text.raw
            && raw.len() <= self.output.len() - start
        {
            self.output.truncate(start);
            self.output.print_bytes(raw.as_bytes());
        }
    }

    /// Starts with a digit or a `.`.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn number(&mut self, text: &str) {
        let merges = if text.starts_with('.') { NUMBER | DOT_DOT } else { WORD | NUMBER };
        self.emit(text.as_bytes(), merges, NUMBER);
    }

    /// Starts with a quote or a long bracket's `[`, and ends with nothing
    /// anything merges with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn string(&mut self, text: &[u8]) {
        let merges = if text.first() == Some(&b'[') { LEFT_BRACKET } else { OTHER };
        self.emit(text, merges, OTHER);
    }

    fn write_string_spelling(&mut self, spelling: StringSpelling<'_>) {
        if spelling.is_long_bracket() {
            self.space_before(LEFT_BRACKET);
        }
        self.output.print_with(|output| spelling.write_to(output));
        self.last = OTHER;
    }
}

/// Whether `raw` is a decimal integer that no spelling of its value beats,
/// as most numerals are.
///
/// That's twelve digits at most, which hex can't shorten, no leading zero,
/// and, where every number is a float, too few trailing zeros for an
/// exponent to pay off.
fn is_shortest_numeral(raw: &str, version: LuaVersion) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() <= 12
        && bytes.iter().all(u8::is_ascii_digit)
        && (bytes.len() == 1 || bytes.first() != Some(&b'0'))
        && (version.has_integer_subtype()
            || bytes.iter().rev().take_while(|&&b| b == b'0').count() < 3)
}

/// `raw` when it's no longer than `spelling`.
fn shorter<'text>(raw: Option<&'text str>, spelling: &'text str) -> &'text str {
    raw.filter(|written| written.len() <= spelling.len()).unwrap_or(spelling)
}
