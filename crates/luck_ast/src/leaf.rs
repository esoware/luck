use core::fmt;
use core::num::NonZeroU32;

use luck_allocator::ThinStr;
use luck_token::Span;

/// An identifier: a variable, a field, a parameter, a label, a type, or an
/// attribute. A name the parser found missing has empty text.
///
/// Its [`NameIndex`] is a pass's numbering kept on the node, so `==` and
/// `Debug` leave it out, and two names are equal when their text and span
/// are.
#[derive(Clone, Copy)]
// In this order the text's length and the index share a word, between an
// aligned pointer and an aligned span, which parses measurably faster.
#[repr(C)]
pub struct Name<'ast> {
    pub text: ThinStr<'ast>,
    pub index: Option<NameIndex>,
    pub span: Span,
}

impl PartialEq for Name<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.span == other.span
    }
}

impl Eq for Name<'_> {}

impl fmt::Debug for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Name").field("text", &self.text).field("span", &self.span).finish()
    }
}

/// A name's index into the tables of the pass that numbered it.
///
/// The parser numbers the names it reads from 0, in reading order, and a
/// pass that holds the tree mutably may number them its own way. An index
/// past `u32::MAX - 1` saturates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NameIndex(NonZeroU32);

impl NameIndex {
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self(NonZeroU32::MIN.saturating_add(index))
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get().wrapping_sub(1)
    }
}

impl fmt::Debug for NameIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NameIndex({})", self.get())
    }
}

/// A number, an integer, a string, or a piece of an interpolated string, as
/// written.
///
/// A string keeps its quotes or long brackets and its escapes, and an
/// interpolated piece is what lies between its delimiters. The node holding it
/// says which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Literal<'ast> {
    pub text: &'ast str,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_saturates_below_u32_max() {
        let last = u32::MAX.wrapping_sub(1);
        for (index, kept) in [(0, 0), (7, 7), (last, last), (u32::MAX, last)] {
            assert_eq!(NameIndex::new(index).get(), kept, "{index}");
        }
        assert_eq!(format!("{:?}", NameIndex::new(3)), "NameIndex(3)");
    }

    #[test]
    fn equality_and_debug_leave_a_name_s_index_out() {
        let name = Name { text: "x".into(), index: None, span: Span::new(0, 1) };
        let numbered = Name { index: Some(NameIndex::new(0)), ..name };
        assert_eq!(name, numbered);
        assert_eq!(
            format!("{numbered:?}"),
            r#"Name { text: "x", span: Span { start: 0, end: 1 } }"#
        );
    }
}
