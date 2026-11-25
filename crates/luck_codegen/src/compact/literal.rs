use luck_ast::{NumberLiteral, StringLiteral};
use luck_token::{
    StringSpelling, spell_number, spell_string, spell_string_shorter_than, write_interpolated_text,
};

use super::Printer;
use crate::separator::{DOT_DOT, LEFT_BRACKET, NUMBER, OTHER, WORD};

impl Printer {
    /// As written, or the shorter of that and the shortest spelling of its
    /// value, as written on a tie.
    pub(super) fn print_number(&mut self, literal: &NumberLiteral<'_>) {
        if let Some(raw) = literal.raw
            && (!self.is_shortest || is_shortest_numeral(raw.as_bytes()))
        {
            self.number(raw.as_bytes());
            return;
        }
        let raw = literal.raw.as_deref();
        let spelling = spell_number(literal.value, raw);
        self.number(shorter(raw.map(str::as_bytes), spelling.as_bytes()));
    }

    /// As [`Self::print_number`] prints a number. No spelling is shorter than
    /// the value in quotes, so a literal no longer than that, as a quoted
    /// one with no escape is, needs no other spelling measured.
    pub(super) fn print_string(&mut self, literal: &StringLiteral<'_>) {
        let Some(raw) = literal.raw else {
            self.write_string_spelling(spell_string(&literal.value));
            return;
        };
        if !self.is_shortest || raw.len() <= literal.value.len() + 2 {
            self.string(raw.as_bytes());
            return;
        }
        match spell_string_shorter_than(&literal.value, raw.len()) {
            Some(spelling) => self.write_string_spelling(spelling),
            None => self.string(raw.as_bytes()),
        }
    }

    /// A piece of an interpolated string's text, which nothing merges with.
    /// Escaping its value never shortens it, so text no longer than its value,
    /// as text with no escape is, stays as written.
    pub(super) fn print_interpolated_text(&mut self, text: &StringLiteral<'_>) {
        if let Some(raw) = text.raw
            && (!self.is_shortest || raw.len() <= text.value.len())
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

    /// Starts with a digit or a `.`. The lexer reads a `.` into a numeral
    /// only before its first letter, so one with a radix prefix, an exponent,
    /// or a suffix ends as a name does, which `space_before` reads from its
    /// text.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn number(&mut self, text: &[u8]) {
        let merges = if text.first() == Some(&b'.') { NUMBER | DOT_DOT } else { WORD | NUMBER };
        self.space_before(merges);
        self.numeral_start = self.output.len();
        self.output.print_bytes(text);
        self.last = NUMBER;
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
/// and too few trailing zeros for an exponent to pay off.
fn is_shortest_numeral(raw: &[u8]) -> bool {
    raw.len() <= 12
        && raw.iter().all(u8::is_ascii_digit)
        && (raw.len() == 1 || raw.first() != Some(&b'0'))
        && raw.iter().rev().take_while(|&&b| b == b'0').count() < 3
}

/// `raw` when it's no longer than `spelling`.
fn shorter<'text>(raw: Option<&'text [u8]>, spelling: &'text [u8]) -> &'text [u8] {
    raw.filter(|written| written.len() <= spelling.len()).unwrap_or(spelling)
}
