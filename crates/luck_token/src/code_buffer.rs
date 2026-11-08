//! Byte-level output builder for the code emitters.
//!
//! Skips per-push UTF-8 machinery for the ASCII punctuation that dominates
//! generated code, while holding a UTF-8 invariant so the final `String`
//! conversion is zero-copy.

use core::iter;

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
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self { bytes: Vec::with_capacity(capacity) }
    }

    #[inline]
    #[must_use]
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Push a single ASCII byte.
    ///
    /// The assertion compiles away for a literal byte, and the grow path is
    /// outlined so a push with spare capacity stays a load, store, and
    /// increment.
    ///
    /// # Panics
    ///
    /// Panics if `byte` is not ASCII.
    #[inline]
    pub fn print_ascii_byte(&mut self, byte: u8) {
        #[cold]
        #[inline(never)]
        fn push_slow(bytes: &mut Vec<u8>, byte: u8) {
            bytes.push(byte);
        }

        assert!(byte.is_ascii(), "byte {byte} is not ASCII");

        if self.bytes.len() < self.bytes.capacity() {
            self.bytes.push(byte);
        } else {
            push_slow(&mut self.bytes, byte);
        }
    }

    /// Push `count` copies of an ASCII byte (indentation runs).
    ///
    /// # Panics
    ///
    /// Panics if `byte` is not ASCII.
    #[inline]
    pub fn print_ascii_repeat(&mut self, byte: u8, count: usize) {
        assert!(byte.is_ascii(), "byte {byte} is not ASCII");
        self.bytes.extend(iter::repeat_n(byte, count));
    }

    #[inline]
    pub fn print_str(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
    }

    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Shorten the buffer to `new_length` bytes. The cut must land on a UTF-8
    /// boundary (always true when the removed bytes are ASCII).
    ///
    /// # Panics
    ///
    /// Panics if the cut would split a UTF-8 sequence.
    #[inline]
    pub fn truncate(&mut self, new_length: usize) {
        assert!(
            self.bytes.get(new_length).is_none_or(|byte| byte & 0xC0 != 0x80),
            "truncation must land on a UTF-8 boundary"
        );
        self.bytes.truncate(new_length);
    }

    #[inline]
    #[must_use]
    #[expect(
        unsafe_code,
        reason = "valid UTF-8 by construction; the safe form would revalidate all output"
    )]
    pub fn into_string(self) -> String {
        debug_assert!(str::from_utf8(&self.bytes).is_ok(), "CodeBuffer must hold valid UTF-8");
        // SAFETY: Every method keeps `bytes` valid UTF-8, and `truncate`
        // asserts that its cut lands on a character boundary.
        unsafe { String::from_utf8_unchecked(self.bytes) }
    }
}

#[cfg(test)]
mod tests {
    use core::panic::AssertUnwindSafe;
    use std::panic;

    use super::*;

    #[test]
    fn pushes_append_mixed_ascii_and_unicode_in_order() {
        let mut buffer = CodeBuffer::with_capacity(0);
        assert!(buffer.is_empty());
        buffer.print_ascii_byte(b'=');
        buffer.print_str("local x");
        buffer.print_str("\u{00e9}");
        buffer.print_ascii_repeat(b' ', 3);
        assert_eq!(buffer.len(), 13);
        assert_eq!(buffer.into_string(), "=local x\u{00e9}   ");
    }

    #[test]
    fn truncate_drops_trailing_bytes() {
        let mut buffer = CodeBuffer::default();
        buffer.print_str("abc  ");
        assert_eq!(buffer.as_bytes(), b"abc  ");
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
            assert_eq!(buffer.into_string(), text.split_at(offset).0);
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
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                buffer.truncate(offset);
            }));
            assert!(result.is_err(), "offset {offset}");
            assert_eq!(buffer.into_string(), text);
        }
    }

    #[test]
    fn ascii_pushes_reject_non_ascii_bytes_without_changing_output() {
        let rejects_as_not_ascii = |push: &mut dyn FnMut()| {
            panic::catch_unwind(AssertUnwindSafe(push)).is_err_and(|payload| {
                payload
                    .downcast_ref::<String>()
                    .is_some_and(|message| message.contains("not ASCII"))
            })
        };
        for byte in [0x80, 0xC3, 0xFF] {
            let mut buffer = CodeBuffer::default();
            buffer.print_str("ok");
            assert!(rejects_as_not_ascii(&mut || buffer.print_ascii_byte(byte)), "byte {byte}");
            assert!(
                rejects_as_not_ascii(&mut || buffer.print_ascii_repeat(byte, 2)),
                "byte {byte}"
            );
            assert_eq!(buffer.into_string(), "ok");
        }
    }
}
