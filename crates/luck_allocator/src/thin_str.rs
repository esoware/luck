use std::{
    fmt::{self, Debug, Display, Formatter},
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull,
    slice, str,
};

/// A `&str` in 12 bytes, aligned to 4: a pointer and a 32-bit length, packed so that a node holding
/// one can put a 4-byte field beside it where a `&str` would leave 4 bytes of padding.
///
/// It derefs to its `str`, and [`as_str`](ThinStr::as_str) gives the `&'a str` back.
#[derive(Clone, Copy)]
#[repr(C, packed(4))]
pub struct ThinStr<'a> {
    ptr: NonNull<u8>,
    len: u32,
    marker: PhantomData<&'a str>,
}

/// SAFETY: A `ThinStr` is a `&str`, which is `Send`.
unsafe impl Send for ThinStr<'_> {}

/// SAFETY: A `ThinStr` is a `&str`, which is `Sync`.
unsafe impl Sync for ThinStr<'_> {}

impl<'a> ThinStr<'a> {
    /// # Panics
    ///
    /// Panics if `text` is 4 GiB or longer, which no source luck reads is.
    #[must_use]
    #[inline]
    pub const fn new(text: &'a str) -> Self {
        assert!(text.len() <= u32::MAX as usize, "a thin str holds under 4 GiB");
        #[expect(clippy::cast_possible_truncation, reason = "checked above")]
        let len = text.len() as u32;
        Self { ptr: NonNull::from_ref(text).cast(), len, marker: PhantomData }
    }

    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        let (ptr, len) = (self.ptr, self.len);
        // SAFETY: `ptr` and `len` came from a `&'a str` in `new`, and `u32` fits in `usize` on
        // every target luck builds for.
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(ptr.as_ptr(), len as usize)) }
    }
}

impl Deref for ThinStr<'_> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl<'a> From<&'a str> for ThinStr<'a> {
    #[inline]
    fn from(text: &'a str) -> Self {
        Self::new(text)
    }
}

impl PartialEq for ThinStr<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for ThinStr<'_> {}

impl PartialEq<str> for ThinStr<'_> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ThinStr<'_> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Debug for ThinStr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}

impl Display for ThinStr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}
