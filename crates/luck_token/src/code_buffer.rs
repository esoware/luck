//! Byte-level output builder for the code emitters. Skips per-push UTF-8
//! machinery for the ASCII punctuation that dominates generated code, while
//! holding a UTF-8 invariant so the final `String` conversion is zero-copy.

/// A string builder over a byte buffer.
///
/// INVARIANT: `bytes` is valid UTF-8 at every public-method boundary. All
/// safe methods preserve it, so `into_string` can skip re-validation.
#[derive(Debug, Default)]
pub struct CodeBuffer {
    bytes: Vec<u8>,
}

impl CodeBuffer {
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Push a single ASCII byte. The assertion compiles away when the byte
    /// is a literal; the grow path is outlined so the common
    /// sufficient-capacity push stays a load, store, and increment.
    #[inline]
    pub fn print_ascii_byte(&mut self, byte: u8) {
        assert!(byte.is_ascii(), "byte {byte} is not ASCII");

        #[cold]
        #[inline(never)]
        fn push_slow(bytes: &mut Vec<u8>, byte: u8) {
            bytes.push(byte);
        }

        if self.bytes.len() < self.bytes.capacity() {
            self.bytes.push(byte);
        } else {
            push_slow(&mut self.bytes, byte);
        }
    }

    /// Push `count` copies of an ASCII byte (indentation runs).
    #[inline]
    pub fn print_ascii_repeat(&mut self, byte: u8, count: usize) {
        assert!(byte.is_ascii(), "byte {byte} is not ASCII");
        self.bytes.extend(std::iter::repeat_n(byte, count));
    }

    #[inline]
    pub fn print_char(&mut self, ch: char) {
        let mut encoded = [0; 4];
        self.bytes
            .extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
    }

    #[inline]
    pub fn print_str(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn last_byte(&self) -> Option<u8> {
        self.bytes.last().copied()
    }

    /// Shorten the buffer to `new_len` bytes. The cut must land on a UTF-8
    /// boundary (always true when the removed bytes are ASCII).
    #[inline]
    pub fn truncate(&mut self, new_len: usize) {
        assert!(
            self.bytes
                .get(new_len)
                .is_none_or(|byte| byte & 0xC0 != 0x80),
            "truncation must land on a UTF-8 boundary"
        );
        self.bytes.truncate(new_len);
    }

    #[must_use]
    #[inline]
    pub fn into_string(self) -> String {
        if cfg!(debug_assertions) {
            String::from_utf8(self.bytes).expect("CodeBuffer must hold valid UTF-8")
        } else {
            // SAFETY: Every safe method preserves the UTF-8 invariant,
            // including the boundary assertion in `truncate`.
            unsafe { String::from_utf8_unchecked(self.bytes) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mixed_ascii_and_unicode() {
        let mut buffer = CodeBuffer::with_capacity(16);
        buffer.print_str("local x");
        buffer.print_ascii_byte(b'=');
        buffer.print_char('\u{00e9}');
        buffer.print_ascii_repeat(b' ', 3);
        assert_eq!(buffer.len(), 13);
        assert_eq!(buffer.into_string(), "local x=\u{00e9}   ");
    }

    #[test]
    fn truncate_drops_trailing_bytes() {
        let mut buffer = CodeBuffer::default();
        buffer.print_str("abc  ");
        assert_eq!(buffer.last_byte(), Some(b' '));
        buffer.truncate(3);
        assert_eq!(buffer.into_string(), "abc");
    }

    #[test]
    fn truncate_accepts_every_character_boundary() {
        let text = "a\u{e9}\u{6f22}\u{1f600}z";
        for offset in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let mut buffer = CodeBuffer::default();
            buffer.print_str(text);
            buffer.truncate(offset);
            assert_eq!(buffer.into_string(), &text[..offset]);
        }
        for offset in [text.len() + 1, usize::MAX] {
            let mut buffer = CodeBuffer::default();
            buffer.print_str(text);
            buffer.truncate(offset);
            assert_eq!(buffer.into_string(), text);
        }
    }

    #[test]
    fn truncate_rejects_splitting_a_character_without_changing_output() {
        let text = "a\u{e9}\u{6f22}\u{1f600}z";
        for offset in (0..text.len()).filter(|offset| !text.is_char_boundary(*offset)) {
            let mut buffer = CodeBuffer::default();
            buffer.print_str(text);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                buffer.truncate(offset);
            }));
            assert!(result.is_err(), "offset {offset}");
            assert_eq!(buffer.into_string(), text);
        }
    }

    #[test]
    #[should_panic(expected = "not ASCII")]
    fn rejects_non_ascii_byte() {
        let mut buffer = CodeBuffer::default();
        buffer.print_ascii_byte(0xFF);
    }
}
