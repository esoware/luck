mod expr;
mod stmt;
mod types;

use luck_ast::Block;
use luck_token::CodeBuffer;

use crate::separator::{DOT_DOT, LEFT_BRACKET, NUMBER, OTHER, WORD, class_of, merges_after};

/// The output starts with no reserved capacity. Use
/// [`compact_with_capacity`] when there's a useful size estimate.
#[must_use]
pub fn compact(block: &Block<'_>) -> String {
    compact_with_capacity(block, 0)
}

/// `capacity` bytes are reserved up front, and one the allocator can't meet
/// is dropped, so the output never depends on it. A parsed tree's source
/// length makes a good hint: compact output is rarely longer.
#[must_use]
pub fn compact_with_capacity(block: &Block<'_>, capacity: usize) -> String {
    let mut printer = Printer { output: CodeBuffer::with_capacity(capacity), last: OTHER };
    printer.print_block(block);
    printer.output.into_string()
}

struct Printer {
    output: CodeBuffer,
    /// The class of the last piece written, from `separator`.
    last: u8,
}

impl Printer {
    /// Writes `text`, after a space when the last piece's class is one of
    /// `merges`, and leaves `class` as the last.
    #[expect(
        clippy::inline_always,
        reason = "a spelling known at compile time folds its checks away only once inlined"
    )]
    #[inline(always)]
    fn emit(&mut self, text: &str, merges: u8, class: u8) {
        if self.last & merges != OTHER {
            self.output.print_ascii_byte(b' ');
        }
        self.output.print_str(text);
        self.last = class;
    }

    /// A keyword, a symbol, or an operator, whose first and last bytes say
    /// what it merges with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn piece(&mut self, text: &str) {
        if let (Some(&first), Some(&last)) = (text.as_bytes().first(), text.as_bytes().last()) {
            self.emit(text, merges_after(first), class_of(last));
        }
    }

    /// Merges as a keyword does, whatever letter or `_` it starts with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn name(&mut self, text: &str) {
        self.emit(text, WORD | NUMBER, WORD);
    }

    /// Starts with a digit or a `.`.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn number(&mut self, text: &str) {
        let merges = if text.starts_with('.') { NUMBER | DOT_DOT } else { WORD | NUMBER };
        self.emit(text, merges, NUMBER);
    }

    /// Starts with a quote or a long bracket's `[`, and ends with nothing
    /// anything merges with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn string(&mut self, text: &str) {
        let merges = if text.starts_with('[') { LEFT_BRACKET } else { OTHER };
        self.emit(text, merges, OTHER);
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
