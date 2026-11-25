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
        // SAFETY: `new` took the bytes from a `&'a str`, `from_ascii` checked they're ASCII, and
        // `TextBytes::text` checked they're ASCII, or a run of UTF-8 that starts and ends with an
        // ASCII byte.
        unsafe { str::from_utf8_unchecked(self.bytes.as_bytes()) }
    }

    #[must_use]
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes.as_bytes()
    }
}

/// Bytes checked once, as a whole source can be, so a range of them that starts and ends with an
/// ASCII byte, as a name or a numeral does, reads as text with the least check of its own they
/// allow: none when they're ASCII, its first and last bytes when they're UTF-8, and every byte of
/// it otherwise.
#[derive(Clone, Copy, Debug)]
pub struct TextBytes<'a> {
    bytes: &'a [u8],
    encoding: Encoding,
}

/// What the bytes of a [`TextBytes`] were checked to be.
#[derive(Clone, Copy, Debug)]
enum Encoding {
    Ascii,
    Utf8,
    /// Neither, so a range of them is text only when each of its bytes is ASCII.
    Other,
}

impl<'a> TextBytes<'a> {
    #[must_use]
    #[inline]
    pub fn new(bytes: &'a [u8]) -> Self {
        // Checking for ASCII costs a fraction of validating UTF-8, and most sources are ASCII.
        let encoding = if bytes.is_ascii() {
            Encoding::Ascii
        } else if str::from_utf8(bytes).is_ok() {
            Encoding::Utf8
        } else {
            Encoding::Other
        };
        Self { bytes, encoding }
    }

    /// `range` of the bytes as text, or `None` when it's out of them, or doesn't start and end
    /// with an ASCII byte, or holds a byte that isn't ASCII in bytes that aren't UTF-8.
    #[must_use]
    #[inline]
    pub fn text(self, range: Range<usize>) -> Option<ThinStr<'a>> {
        let bytes = self.bytes.get(range)?;
        let is_text = match self.encoding {
            Encoding::Ascii => true,
            // In UTF-8, an ASCII byte is a whole character, so a run that starts and ends with
            // one starts and ends between characters, and holds only whole ones.
            Encoding::Utf8 => match (bytes.first(), bytes.last()) {
                (Some(first), Some(last)) => (first | last).is_ascii(),
                _ => true,
            },
            Encoding::Other => is_ascii_out_of_line(bytes),
        };
        is_text.then_some(ThinStr { bytes: ThinBytes::new(bytes) })
    }
}

/// [`is_ascii`] for bytes that are seldom checked this way, kept out of the callers of
/// [`TextBytes::text`], which run on every name.
#[inline(never)]
fn is_ascii_out_of_line(bytes: &[u8]) -> bool {
    is_ascii(bytes)
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
