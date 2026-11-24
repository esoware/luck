#![expect(unsafe_code, reason = "a thin str reads its bytes as UTF-8 without checking them again")]

use std::{
    fmt::{self, Debug, Display, Formatter},
    ops::{Deref, Range},
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
        is_ascii(bytes).then(|| Self { bytes: ThinBytes::new(bytes) })
    }

    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'a str {
        // SAFETY: `new` took the bytes from a `&'a str`, and `from_ascii` and
        // `AsciiBytes::new` checked they're ASCII.
        unsafe { str::from_utf8_unchecked(self.bytes.as_bytes()) }
    }

    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes.as_bytes()
    }
}

/// Bytes checked once to be ASCII, as a whole source can be, so any range of
/// them is text with no check of its own.
#[derive(Clone, Copy, Debug)]
pub struct AsciiBytes<'a>(&'a [u8]);

impl<'a> AsciiBytes<'a> {
    #[must_use]
    #[inline]
    pub fn new(bytes: &'a [u8]) -> Option<Self> {
        bytes.is_ascii().then_some(Self(bytes))
    }

    /// Empty if `range` is out of the bytes.
    #[must_use]
    #[inline]
    pub fn text(self, range: Range<usize>) -> ThinStr<'a> {
        ThinStr { bytes: ThinBytes::new(self.0.get(range).unwrap_or_default()) }
    }
}

/// `[u8]::is_ascii`, but a slice of at most 16 bytes, as nearly every name
/// and numeral is, is tested as two overlapping words of one width, which
/// measured faster on them.
#[inline]
fn is_ascii(bytes: &[u8]) -> bool {
    match bytes.len() {
        8..=16 => {
            let (Some(head), Some(tail)) = (bytes.first_chunk(), bytes.last_chunk()) else {
                return false;
            };
            (u64::from_ne_bytes(*head) | u64::from_ne_bytes(*tail)) & 0x8080_8080_8080_8080 == 0
        }
        4..=7 => {
            let (Some(head), Some(tail)) = (bytes.first_chunk(), bytes.last_chunk()) else {
                return false;
            };
            (u32::from_ne_bytes(*head) | u32::from_ne_bytes(*tail)) & 0x8080_8080 == 0
        }
        2 | 3 => {
            let (Some(head), Some(tail)) = (bytes.first_chunk(), bytes.last_chunk()) else {
                return false;
            };
            (u16::from_ne_bytes(*head) | u16::from_ne_bytes(*tail)) & 0x8080 == 0
        }
        _ => bytes.is_ascii(),
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
