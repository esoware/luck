use std::{
    fmt::{self, Debug, Formatter, Write},
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull,
    slice,
};

/// A `&[u8]` in 12 bytes, aligned to 4: a pointer and a 32-bit length, packed so that a node
/// holding one can put a 4-byte field beside it where a `&[u8]` would leave 4 bytes of padding.
///
/// It derefs to its bytes, and [`as_bytes`](ThinBytes::as_bytes) gives the `&'a [u8]` back.
/// `Debug` writes it as a quoted string, with each byte that isn't UTF-8 as a `\x` escape.
#[derive(Clone, Copy)]
#[repr(C, packed(4))]
pub struct ThinBytes<'a> {
    ptr: NonNull<u8>,
    len: u32,
    marker: PhantomData<&'a [u8]>,
}

/// SAFETY: A `ThinBytes` is a `&[u8]`, which is `Send`.
unsafe impl Send for ThinBytes<'_> {}

/// SAFETY: A `ThinBytes` is a `&[u8]`, which is `Sync`.
unsafe impl Sync for ThinBytes<'_> {}

impl<'a> ThinBytes<'a> {
    /// # Panics
    ///
    /// Panics if `bytes` is 4 GiB or longer, which no source luck reads is.
    #[must_use]
    #[inline]
    pub const fn new(bytes: &'a [u8]) -> Self {
        assert!(bytes.len() <= u32::MAX as usize, "thin bytes hold under 4 GiB");
        #[expect(clippy::cast_possible_truncation, reason = "checked above")]
        let len = bytes.len() as u32;
        Self { ptr: NonNull::from_ref(bytes).cast(), len, marker: PhantomData }
    }

    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        let (ptr, len) = (self.ptr, self.len);
        // SAFETY: `ptr` and `len` came from a `&'a [u8]` in `new`, and `u32` fits in `usize` on
        // every target luck builds for.
        unsafe { slice::from_raw_parts(ptr.as_ptr(), len as usize) }
    }
}

impl Deref for ThinBytes<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl<'a> From<&'a [u8]> for ThinBytes<'a> {
    #[inline]
    fn from(bytes: &'a [u8]) -> Self {
        Self::new(bytes)
    }
}

impl<'a> From<&'a str> for ThinBytes<'a> {
    #[inline]
    fn from(text: &'a str) -> Self {
        Self::new(text.as_bytes())
    }
}

impl PartialEq for ThinBytes<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for ThinBytes<'_> {}

impl PartialEq<[u8]> for ThinBytes<'_> {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_bytes() == other
    }
}

impl PartialEq<&[u8]> for ThinBytes<'_> {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_bytes() == *other
    }
}

impl Hash for ThinBytes<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl Debug for ThinBytes<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for chunk in self.as_bytes().utf8_chunks() {
            // A `str` leaves its single quotes as they are, as `char` doesn't.
            for c in chunk.valid().chars() {
                if c == '\'' { f.write_char(c)? } else { write!(f, "{}", c.escape_debug())? }
            }
            for byte in chunk.invalid() {
                write!(f, "\\x{byte:02X}")?;
            }
        }
        f.write_char('"')
    }
}
