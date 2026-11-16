//! Skips per-push UTF-8 machinery for the ASCII punctuation that dominates
//! generated code, while holding a UTF-8 invariant so the final `String`
//! conversion is zero-copy.

use core::iter;

/// INVARIANT: `bytes` is valid UTF-8 at every public-method boundary. All
/// safe methods preserve it, so `into_string` can skip re-validation.
#[derive(Debug, Default)]
pub struct CodeBuffer {
    bytes: Vec<u8>,
}

impl CodeBuffer {
    /// The capacity is a hint: one the allocator can't meet is dropped, and
    /// the buffer grows as it's written.
    #[inline]
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let mut bytes = Vec::new();
        let _unused = bytes.try_reserve_exact(capacity);
        Self { bytes }
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

    /// The cut must land on a UTF-8 boundary (always true when the removed bytes are ASCII).
    ///
    /// # Panics
    ///
    /// Panics if the cut would split a UTF-8 sequence.
    #[inline]
    pub fn truncate(&mut self, new_length: usize) {
        assert!(
            self.bytes.get(new_length).is_none_or(|b| b & 0xC0 != 0x80),
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
