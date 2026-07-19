/// Zero-copy forward-only cursor over source bytes. Tracks byte position.
pub struct Cursor<'src> {
    source: &'src [u8],
    position: usize,
}

impl<'src> Cursor<'src> {
    pub fn new(source: &'src [u8]) -> Self {
        Self {
            source,
            position: 0,
        }
    }

    #[inline]
    pub fn position(&self) -> usize {
        self.position
    }

    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.source.get(self.position).copied()
    }

    #[inline]
    pub fn peek_at(&self, offset: usize) -> Option<u8> {
        self.source.get(self.position + offset).copied()
    }

    #[inline]
    pub fn advance(&mut self) -> Option<u8> {
        let byte = self.source.get(self.position).copied()?;
        self.position += 1;
        Some(byte)
    }

    #[inline]
    pub fn rest(&self) -> &'src [u8] {
        &self.source[self.position..]
    }

    #[inline]
    pub fn advance_by(&mut self, count: usize) {
        debug_assert!(self.position + count <= self.source.len());
        self.position += count;
    }

    /// Advance to the first byte matching `table`, or EOF.
    #[inline]
    pub fn advance_until_match(&mut self, table: &crate::search::ByteMatchTable) {
        self.position += crate::search::find_match(self.rest(), table);
    }

    pub fn eat_while(&mut self, predicate: impl Fn(u8) -> bool) -> &'src [u8] {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if predicate(byte) {
                self.position += 1;
            } else {
                break;
            }
        }
        &self.source[start..self.position]
    }
}
