mod expr;
mod literal;
mod stmt;
mod types;

use luck_ast::Block;
use luck_data_structures::CodeBuffer;

use crate::separator::{Classes, DOT_DOT, NUMBER, OTHER, WORD, class_of, merges_after};

/// How [`compact`] spells each literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralSpelling {
    /// As the tree spells it, so a parsed tree's literals print as its source
    /// wrote them, and one built in code the shortest way its value reads
    /// back.
    AsWritten,
    /// The shortest way its value reads back, or as the tree spells it when
    /// that's no longer, as a minifier wants.
    Shortest,
}

/// The output is bytes, as a source is, and starts with as much capacity as
/// the chunk spans, a parsed tree's source but for what lies outside its
/// statements, which compact output is rarely longer than. Use
/// [`compact_into`] to write after a header, or into a buffer of another
/// size.
pub fn compact(chunk: &Block<'_>, spelling: LiteralSpelling) -> Vec<u8> {
    let mut output = Vec::new();
    // A tree built in code can span anything, so a hint the allocator can't
    // meet is dropped.
    let _ = output.try_reserve_exact(chunk.span.len() as usize);
    compact_into(chunk, spelling, output)
}

/// Writes `chunk` after the bytes `output` holds, in its buffer.
///
/// A header a caller writes first, such as a shebang, costs no copy of the
/// code, and the capacity it reserves is the capacity the printer starts
/// with. A parsed tree's source length, its comments aside, makes a good
/// hint, since compact output is rarely longer.
pub fn compact_into(chunk: &Block<'_>, spelling: LiteralSpelling, output: Vec<u8>) -> Vec<u8> {
    let mut printer = Printer {
        output: CodeBuffer::from(output),
        last: OTHER,
        numeral_start: 0,
        is_shortest: spelling == LiteralSpelling::Shortest,
    };
    printer.print_block(chunk);
    printer.output.into_bytes()
}

/// Each kind of node prints in a method of its own, reached from a `match`
/// arm.
///
/// An unoptimized build inlines nothing but what's forced and keeps every
/// local of a function in a slot of its own, so a method that printed
/// several kinds of node would take all their slots at every level of
/// nesting.
struct Printer {
    output: CodeBuffer,
    /// The class of the last piece written.
    last: Classes,
    /// Where the last numeral starts in the output, which is where its text
    /// is while it's the last piece.
    numeral_start: usize,
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
    fn emit(&mut self, text: &[u8], merges: Classes, class: Classes) {
        self.space_before(merges);
        self.output.print_bytes(text);
        self.last = class;
    }

    /// A numeral past its first letter ends as a name does, so a piece that
    /// starts with `.` glues to it, which its text alone says.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn space_before(&mut self, merges: Classes) {
        if self.last.intersects(merges)
            && !(merges.intersects(DOT_DOT)
                && self.last == NUMBER
                && self.output.as_bytes()[self.numeral_start..].iter().any(u8::is_ascii_alphabetic))
        {
            self.output.print_byte(b' ');
        }
    }

    /// A keyword, a symbol, or an operator, whose first and last bytes say
    /// what it merges with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn piece(&mut self, text: &str) {
        let bytes = text.as_bytes();
        self.emit(bytes, merges_after(bytes[0]), class_of(bytes[bytes.len() - 1]));
    }

    /// Merges as a keyword does, whatever letter or `_` it starts with.
    #[expect(clippy::inline_always, reason = "see `emit`")]
    #[inline(always)]
    fn name(&mut self, text: &str) {
        self.emit(text.as_bytes(), WORD | NUMBER, WORD);
    }

    /// Separates `items` with commas, a piece known at compile time wherever
    /// the list prints, even where it isn't inlined.
    fn list<T>(&mut self, items: &[T], mut print_item: impl FnMut(&mut Self, &T)) {
        if let Some((first, rest)) = items.split_first() {
            print_item(self, first);
            for item in rest {
                self.piece(",");
                print_item(self, item);
            }
        }
    }
}
