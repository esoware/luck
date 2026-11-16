mod declaration;
mod expr;
mod function;
mod stmt;
mod types;

use luck_ast::{Block, Punctuated};
use luck_token::{CodeBuffer, Token, TokenKind};

use crate::separator::{PieceClass, needs_space};

/// The output starts with no reserved capacity. Use
/// [`compact_with_capacity`] when there's a useful size estimate.
#[must_use]
pub fn compact(block: &Block) -> String {
    compact_with_capacity(block, 0)
}

/// The hint only affects allocation, never the output, and one the allocator
/// can't meet is dropped.
///
/// A previous output's length makes a good one. Source length over-reserves for comment-heavy
/// input, and a transformed AST can print longer than its source.
#[must_use]
pub fn compact_with_capacity(block: &Block, capacity: usize) -> String {
    let mut printer =
        CompactPrinter { output: CodeBuffer::with_capacity(capacity), previous: PieceClass::Other };
    printer.emit_block(block);
    printer.output.into_string()
}

struct CompactPrinter {
    output: CodeBuffer,
    previous: PieceClass,
}

impl CompactPrinter {
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

    /// Every kind prints what the formatter prints for it: the text it
    /// carries, or its fixed spelling.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "every other kind has a fixed spelling, which static_text matches exhaustively"
    )]
    fn emit_token(&mut self, token: &Token) {
        match &token.kind {
            TokenKind::Identifier(text) => self.emit_piece(text, PieceClass::Word),
            TokenKind::Number(text) | TokenKind::Integer(text) => {
                self.emit_piece(text, PieceClass::Number);
            }
            TokenKind::StringLiteral(text)
            | TokenKind::InterpolatedStringBegin(text)
            | TokenKind::InterpolatedStringMiddle(text)
            | TokenKind::InterpolatedStringEnd(text) => self.emit_piece(text, PieceClass::Other),
            // `Eof` has no spelling, so it prints nothing.
            fixed => self.emit_fixed(fixed.static_text().unwrap_or_default()),
        }
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
