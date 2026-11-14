mod declaration;
mod expr;
mod function;
mod stmt;
mod types;

use luck_ast::{Block, MAX_NESTING_DEPTH, Punctuated, name_spelling};
use luck_token::{CodeBuffer, SourceError, Token};

use crate::separator::{PieceClass, needs_space};

/// The output starts with no reserved capacity. Use
/// [`compact_with_capacity`] when there's a useful size estimate.
///
/// # Errors
///
/// Returns an error for a tree nested deeper than [`MAX_NESTING_DEPTH`], and
/// for one holding a leaf with no text that reads back as itself, such as a
/// keyword token where a name belongs or an error node. `luck_parser` builds
/// neither, and its output would read back as some other tree.
pub fn compact(block: &Block) -> Result<String, SourceError> {
    compact_with_capacity(block, 0)
}

/// The hint only affects allocation, never the output, and one the allocator
/// can't meet is dropped.
///
/// A previous output's length makes a good one. Source length over-reserves for comment-heavy
/// input, and a transformed AST can print longer than its source.
///
/// # Errors
///
/// As [`compact`] does.
pub fn compact_with_capacity(block: &Block, capacity: usize) -> Result<String, SourceError> {
    let mut printer = CompactPrinter {
        output: CodeBuffer::with_capacity(capacity),
        previous: PieceClass::Other,
        depth: 0,
        is_too_deep: false,
        unprintable: None,
    };
    printer.emit_block(block);
    if printer.is_too_deep {
        return Err(SourceError {
            span: block.span,
            message: format!(
                "maximum nesting depth exceeded (luck resource limit: {MAX_NESTING_DEPTH})"
            ),
        });
    }
    if let Some(error) = printer.unprintable {
        return Err(error);
    }
    Ok(printer.output.into_string())
}

struct CompactPrinter {
    output: CodeBuffer,
    previous: PieceClass,
    /// Levels open where the printer is, as [`MAX_NESTING_DEPTH`] counts them.
    depth: usize,
    is_too_deep: bool,
    /// The first leaf found with no text that reads back as itself.
    unprintable: Option<SourceError>,
}

impl CompactPrinter {
    /// Enter a level of nesting, the levels `luck_ast::Visitor::descend`
    /// counts. Past the cap nothing more prints, so the printer's own stack
    /// stays bounded on a tree of any depth.
    const fn descend(&mut self) -> bool {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_NESTING_DEPTH {
            self.is_too_deep = true;
        }
        !self.is_too_deep
    }

    const fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Print one piece, preceded by a space if it would otherwise merge with
    /// the previous one.
    #[expect(
        clippy::inline_always,
        reason = "keeps fixed spellings constant through the spacing check and buffer write"
    )]
    #[inline(always)]
    fn emit_piece(&mut self, text: &str, class: PieceClass) {
        let Some(&first_byte) = text.as_bytes().first() else {
            return;
        };
        if needs_space(self.previous, first_byte, class) {
            self.output.print_ascii_byte(b' ');
        }
        self.output.print_str(text);
        self.previous = class;
    }

    #[inline]
    fn emit_fixed(&mut self, text: &'static str) {
        self.emit_piece(text, PieceClass::of_fixed(text));
    }

    /// Print a leaf's text, or keep the error for one that has none.
    fn emit_leaf(&mut self, spelling: Result<&str, SourceError>, class: PieceClass) {
        match spelling {
            Ok(text) => self.emit_piece(text, class),
            Err(error) => self.refuse(error),
        }
    }

    fn refuse(&mut self, error: SourceError) {
        self.unprintable.get_or_insert(error);
    }

    fn emit_name(&mut self, token: &Token) {
        self.emit_leaf(name_spelling(token), PieceClass::Word);
    }

    fn emit_joined<T>(
        &mut self,
        items: &[T],
        separator: &'static str,
        mut emit_item: impl FnMut(&mut Self, &T),
    ) {
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                self.emit_fixed(separator);
            }
            emit_item(self, item);
        }
    }

    /// Emit `punctuated` with `separator` between its items, keeping a trailing
    /// separator the source carried. The owning node knows the spelling,
    /// which `Punctuated` doesn't store.
    fn emit_punctuated<T>(
        &mut self,
        punctuated: &Punctuated<T>,
        separator: &'static str,
        emit_item: impl FnMut(&mut Self, &T),
    ) {
        self.emit_joined(&punctuated.items, separator, emit_item);
        if punctuated.has_trailing_separator && !punctuated.is_empty() {
            self.emit_fixed(separator);
        }
    }
}
