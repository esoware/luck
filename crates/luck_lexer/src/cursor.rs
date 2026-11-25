#![expect(
    unsafe_code,
    reason = "the scans move a raw pointer without the bounds checks its invariant makes redundant"
)]

use std::slice;

use luck_token::{Span, strip_line_break};

use crate::search::{name_length, short_string_length};

/// Measured 5-10% faster per token than a slice and an index with checked
/// slicing.
///
/// INVARIANT: `ptr` lies in `source` or at its end, which `end` marks, so the
/// bytes from `ptr` to `end` are the unread rest of the source. A scan
/// advances only by bytes it counted in [`Self::rest`], and
/// [`Self::advance_by`], which takes any count, clamps it.
#[derive(Debug)]
pub(crate) struct Cursor<'source> {
    source: &'source [u8],
    ptr: *const u8,
    end: *const u8,
}

// SAFETY: A cursor only reads the `&'source [u8]` it borrows, which is `Send`.
unsafe impl Send for Cursor<'_> {}

// SAFETY: A cursor only reads the `&'source [u8]` it borrows, which is `Sync`.
unsafe impl Sync for Cursor<'_> {}

impl<'source> Cursor<'source> {
    pub(crate) const fn new(source: &'source [u8]) -> Self {
        let range = source.as_ptr_range();
        Self { source, ptr: range.start, end: range.end }
    }

    #[inline]
    pub(crate) fn position(&self) -> usize {
        self.ptr.addr() - self.source.as_ptr().addr()
    }

    #[inline]
    pub(crate) fn peek(&self) -> Option<u8> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: `ptr` is before `end`, so it points into the source.
        Some(unsafe { *self.ptr })
    }

    #[inline]
    pub(crate) fn peek_next(&self) -> Option<u8> {
        self.rest().get(1).copied()
    }

    /// Pass the next byte, if there is one. After a [`Self::peek`] that found
    /// one, the check folds into the peek's.
    #[inline]
    pub(crate) fn bump(&mut self) {
        if self.ptr != self.end {
            // SAFETY: A byte remains.
            unsafe { self.advance_unchecked(1) }
        }
    }

    #[inline]
    pub(crate) fn eat(&mut self, byte: u8) -> bool {
        let is_next = self.peek() == Some(byte);
        if is_next {
            // SAFETY: `peek` found a byte.
            unsafe { self.advance_unchecked(1) }
        }
        is_next
    }

    #[inline]
    pub(crate) fn rest(&self) -> &'source [u8] {
        let length = self.end.addr() - self.ptr.addr();
        // SAFETY: The bytes from `ptr` to `end` are the end of the source.
        unsafe { slice::from_raw_parts(self.ptr, length) }
    }

    #[inline]
    pub(crate) fn text_since(&self, start: usize) -> &'source [u8] {
        &self.source[start..self.position()]
    }

    /// A plain cast, measured 2-3% faster per token than a checked one.
    #[inline]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "`Lexer::new` refuses a source over `u32::MAX` bytes, so a position fits a `u32`"
    )]
    pub(crate) fn span_since(&self, start: usize) -> Span {
        Span::new(start as u32, self.position() as u32)
    }

    /// Stops at the end of the source, which only a lexer bug would pass.
    #[inline]
    pub(crate) fn advance_by(&mut self, count: usize) {
        let rest_length = self.rest().len();
        debug_assert!(count <= rest_length, "cursor advanced past the end of the source");
        // SAFETY: The count is clamped to what remains.
        unsafe { self.advance_unchecked(count.min(rest_length)) }
    }

    /// # Safety
    ///
    /// At least `count` bytes must remain.
    #[inline]
    unsafe fn advance_unchecked(&mut self, count: usize) {
        debug_assert!(count <= self.rest().len(), "cursor advanced past the end of the source");
        // SAFETY: The caller keeps `ptr` at or before `end`.
        self.ptr = unsafe { self.ptr.add(count) };
    }

    pub(crate) fn advance_to_line_break(&mut self) {
        let rest = self.rest();
        let length = memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len());
        // SAFETY: `length` counts bytes of `rest`.
        unsafe { self.advance_unchecked(length) }
    }

    #[inline]
    pub(crate) fn eat_while(&mut self, mut predicate: impl FnMut(u8) -> bool) {
        let rest = self.rest();
        let length = rest.iter().position(|&b| !predicate(b)).unwrap_or(rest.len());
        // SAFETY: `length` counts bytes of `rest`.
        unsafe { self.advance_unchecked(length) }
    }

    /// Pass the name at the cursor and return it, or nothing if no name byte
    /// is next.
    #[inline]
    pub(crate) fn eat_name(&mut self) -> &'source [u8] {
        let rest = self.rest();
        let length = name_length(rest);
        // SAFETY: `name_length` counts bytes of `rest`.
        let bytes = unsafe { rest.get_unchecked(..length) };
        // SAFETY: `name_length` counts bytes of `rest`.
        unsafe { self.advance_unchecked(length) }
        bytes
    }

    /// Pass the bytes of a short string's body that `quote` opened, up to the
    /// next quote, escape, or line break.
    #[inline]
    pub(crate) fn eat_short_string_run(&mut self, quote: u8) {
        let length = short_string_length(self.rest(), quote);
        // SAFETY: `short_string_length` counts bytes of `rest`.
        unsafe { self.advance_unchecked(length) }
    }

    /// Consume the rest of the UTF-8 character that begins at `start`, one
    /// byte before the cursor, and return it, so a multi-byte character
    /// yields one error that names it. `None` for a byte that begins no
    /// character, which the cursor is past.
    pub(crate) fn eat_rest_of_char(&mut self, start: usize) -> Option<char> {
        let rest = &self.source[start..];
        // No character takes more than 4 bytes.
        let window = rest.get(..4).unwrap_or(rest);
        let character = window.utf8_chunks().next()?.valid().chars().next()?;
        self.advance_by(character.len_utf8() - 1);
        Some(character)
    }

    /// Whether only spaces and tabs stand between the cursor and a line break
    /// or EOF.
    pub(crate) fn is_at_line_end(&self) -> bool {
        let rest = self.rest();
        let blank_length = rest.iter().take_while(|b| matches!(b, b' ' | b'\t')).count();
        let after_blanks = &rest[blank_length..];
        after_blanks.is_empty() || strip_line_break(after_blanks).is_some()
    }
}
