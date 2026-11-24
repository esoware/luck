mod expr;
mod stmt;
mod types;

use luck_ast::{Chunk, IntegerLiteral, NumberLiteral, StringLiteral};
use luck_data_structures::CodeBuffer;
use luck_token::{
    LuaVersion, StringSpelling, spell_integer, spell_number, spell_string,
    spell_string_shorter_than, write_interpolated_text,
};

use crate::separator::{DOT_DOT, LEFT_BRACKET, NUMBER, OTHER, WORD, class_of, merges_after};

/// How [`compact`] spells each literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralSpelling {
    /// As the tree spells it, so a parsed tree's literals print as its source
    /// wrote them, and one built in code the shortest way its value reads
    /// back.
    AsWritten,
    /// The shortest way its value reads back in the chunk's version, or as
    /// the tree spells it when that's no longer, as a minifier wants.
    Shortest,
}

/// The output is bytes, as a source is, and starts with no reserved
/// capacity. Use [`compact_with_capacity`] when there's a useful size
/// estimate.
#[must_use]
pub fn compact(chunk: &Chunk<'_>, spelling: LiteralSpelling) -> Vec<u8> {
    compact_with_capacity(chunk, spelling, 0)
}

/// `capacity` bytes are reserved up front, and one the allocator can't meet
/// is dropped, so the output never depends on it. A parsed tree's source
/// length makes a good hint: compact output is rarely longer.
#[must_use]
pub fn compact_with_capacity(
    chunk: &Chunk<'_>,
    spelling: LiteralSpelling,
    capacity: usize,
) -> Vec<u8> {
    let mut printer = Printer {
        output: CodeBuffer::with_capacity(capacity),
        last: OTHER,
        version: chunk.version,
        is_shortest: spelling == LiteralSpelling::Shortest,
    };
    printer.print_block(&chunk.block);
    printer.output.into_bytes()
}

/// Each node prints in a method of its own, called from a `match` arm.
///
/// An unoptimized build inlines nothing but what's forced and keeps every
/// local of a function in a slot of its own, so a method that printed
/// several kinds of node would take all their slots at every level of
/// nesting. An optimized build inlines each method back into its one caller.
struct Printer {
    output: CodeBuffer,
    /// The class of the last piece written, from `separator`.
    last: u8,
    version: LuaVersion,
    /// Whether literals take [`LiteralSpelling::Shortest`].
    is_shortest: bool,
}

impl Printer {
    /// Writes `text`, after a space when the last piece's class is one of
    /// `merges`, and leaves `class` as the last.
    #[expect(
        clippy::inline_always,
        reason = "a spelling known at compile time folds its checks away only once inlined"
    )]
    #[inline(always)]
    fn emit(&mut self, text: &[u8], merges: u8, class: u8) {
        if self.last & merges != OTHER {
            self.output.print_byte(b' ');
        }
        self.output.print_bytes(text);
        self.last = class;
    }

    /// A keyword, a symbol, or an operator, whose first and last bytes say
    /// what it merges with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn piece(&mut self, text: &str) {
        let bytes = text.as_bytes();
        if let (Some(&first), Some(&last)) = (bytes.first(), bytes.last()) {
            self.emit(bytes, merges_after(first), class_of(last));
        }
    }

    /// Merges as a keyword does, whatever letter or `_` it starts with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn name(&mut self, text: &str) {
        self.emit(text.as_bytes(), WORD | NUMBER, WORD);
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

    /// As written, or the shorter of that and the shortest spelling of its
    /// value, as written on a tie.
    fn print_number(&mut self, literal: &NumberLiteral<'_>) {
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
    fn print_integer(&mut self, literal: &IntegerLiteral<'_>) {
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
    fn print_string(&mut self, literal: &StringLiteral<'_>) {
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

    fn write_string_spelling(&mut self, spelling: StringSpelling<'_>) {
        if spelling.is_long_bracket() && self.last & LEFT_BRACKET != OTHER {
            self.output.print_byte(b' ');
        }
        self.output.print_with(|output| spelling.write_to(output));
        self.last = OTHER;
    }

    /// Luau: a piece of an interpolated string's text, which nothing merges
    /// with.
    fn print_interpolated_text(&mut self, text: &StringLiteral<'_>) {
        if let Some(raw) = text.raw
            && (!self.is_shortest || !raw.contains(&b'\\'))
        {
            self.output.print_bytes(raw.as_bytes());
            return;
        }
        let start = self.output.len();
        self.output.print_with(|output| write_interpolated_text(&text.value, output));
        if let Some(raw) = text.raw
            && raw.len() <= self.output.len().saturating_sub(start)
        {
            self.output.truncate(start);
            self.output.print_bytes(raw.as_bytes());
        }
    }

    fn list<T>(
        &mut self,
        items: &[T],
        separator: &'static str,
        mut print_item: impl FnMut(&mut Self, &T),
    ) {
        if let Some((first, rest)) = items.split_first() {
            print_item(self, first);
            for item in rest {
                self.piece(separator);
                print_item(self, item);
            }
        }
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
