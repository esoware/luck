#![expect(unsafe_code, reason = "a thin str reads its bytes as UTF-8 without checking them again")]

use std::{
    fmt::{self, Debug, Display, Formatter},
    ops::Deref,
    str,
};

use crate::ThinBytes;

/// A `&str` in 12 bytes, aligned to 4, as [`ThinBytes`] is a `&[u8]`.
///
/// It derefs to its `str`, and [`as_str`](ThinStr::as_str) gives the `&'a str` back.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ThinStr<'a> {
    bytes: ThinBytes<'a>,
}

impl<'a> ThinStr<'a> {
    /// # Panics
    ///
    /// Panics if `text` is 4 GiB or longer, which no source luck reads is.
    #[must_use]
    #[inline]
    pub const fn new(text: &'a str) -> Self {
        Self { bytes: ThinBytes::new(text.as_bytes()) }
    }

    /// `bytes` as text, when they're ASCII, as a name or a numeral is. Checking for ASCII costs
    /// less than validating UTF-8.
    #[must_use]
    #[inline]
    pub fn from_ascii(bytes: &'a [u8]) -> Option<Self> {
        bytes.is_ascii().then(|| Self { bytes: ThinBytes::new(bytes) })
    }

    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        // SAFETY: `new` took the bytes from a `&'a str`, and `from_ascii` checked they're ASCII.
        unsafe { str::from_utf8_unchecked(self.bytes.as_bytes()) }
    }

    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes.as_bytes()
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
