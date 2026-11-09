use luck_token::Span;

/// Zero-copy, forward-only byte cursor over source text.
#[derive(Debug)]
pub(crate) struct Cursor<'source> {
    source: &'source str,
    position: usize,
}

impl<'source> Cursor<'source> {
    pub(crate) const fn new(source: &'source str) -> Self {
        Self { source, position: 0 }
    }

    #[inline]
    pub(crate) const fn position(&self) -> usize {
        self.position
    }

    #[inline]
    pub(crate) fn peek(&self) -> Option<u8> {
        self.source.as_bytes().get(self.position).copied()
    }

    #[inline]
    pub(crate) fn peek_next(&self) -> Option<u8> {
        self.source.as_bytes().get(self.position.saturating_add(1)).copied()
    }

    #[inline]
    pub(crate) fn advance(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.advance_by(1);
        Some(byte)
    }

    /// Consume the next byte if it is `byte`.
    #[inline]
    pub(crate) fn eat(&mut self, byte: u8) -> bool {
        let is_next = self.peek() == Some(byte);
        if is_next {
            self.advance_by(1);
        }
        is_next
    }

    #[inline]
    pub(crate) fn rest(&self) -> &'source [u8] {
        self.source.as_bytes().get(self.position..).unwrap_or_default()
    }

    /// Source text from byte index `start` up to the cursor.
    #[inline]
    pub(crate) fn text_since(&self, start: usize) -> &'source str {
        self.source.get(start..self.position).unwrap_or_default()
    }

    /// Span from byte index `start` up to the cursor.
    #[inline]
    pub(crate) fn span_since(&self, start: usize) -> Span {
        Span::from_indices(start, self.position)
    }

    /// The only place the position moves. It never passes the source length,
    /// so the add can't wrap, and this per-byte path can skip saturation.
    #[inline]
    pub(crate) fn advance_by(&mut self, count: usize) {
        debug_assert!(count <= self.rest().len(), "cursor advanced past the end of the source");
        self.position = self.position.wrapping_add(count);
    }

    /// Advance to the next line break, or EOF.
    pub(crate) fn advance_to_line_break(&mut self) {
        let rest = self.rest();
        self.advance_by(memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len()));
    }

    pub(crate) fn eat_while(&mut self, predicate: impl Fn(u8) -> bool) {
        while self.peek().is_some_and(&predicate) {
            self.advance_by(1);
        }
    }

    /// Consume the rest of the UTF-8 sequence that begins at `start` and
    /// return its char, so a multi-byte char yields one error that names it
    /// and lexing resumes on a char boundary.
    pub(crate) fn eat_rest_of_char(&mut self, start: usize) -> Option<char> {
        // Continuation bytes are 0b10xxxxxx.
        self.eat_while(|byte| (byte & 0xC0) == 0x80);
        self.text_since(start).chars().next()
    }

    /// Whether only spaces and tabs stand between the cursor and a line break
    /// or EOF.
    pub(crate) fn is_at_line_end(&self) -> bool {
        let next_content = self.rest().iter().find(|byte| !matches!(byte, b' ' | b'\t'));
        matches!(next_content, Some(b'\n' | b'\r') | None)
    }
}
