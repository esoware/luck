use std::num::NonZeroU32;
use std::{fmt, str};

use luck_allocator::{ThinBytes, ThinStr};
use luck_ast_macros::Ast;
use luck_token::{LuaNumber, Span};

/// An identifier: a variable, a field, a parameter, a label, a type, or an
/// attribute. A name the parser found missing has empty text.
///
/// Its [`NameIndex`] tells it apart from the tree's other names, not from a
/// name in another tree, so `==`, `Debug`, and `ContentEq` leave it out, and
/// two names are equal when their text and span are.
#[derive(Clone, Copy, Ast)]
// In this order the text's length and the index share a word, between an
// aligned pointer and an aligned span, which parses measurably faster.
#[repr(C)]
pub struct Name<'ast> {
    pub text: ThinStr<'ast>,
    #[content_eq(skip)]
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

/// A name's index among the names of its tree, by which semantic analysis
/// finds what it declares, refers to, or labels.
///
/// The parser numbers the names it reads from 0, in reading order, each its
/// own. A pass that holds the tree mutably may number them its own way, but
/// analysis finds the role of only one of the names that share an index. An
/// index past `u32::MAX - 1` saturates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct NameIndex(NonZeroU32);

impl NameIndex {
    pub const fn new(index: u32) -> Self {
        Self(NonZeroU32::MIN.saturating_add(index))
    }

    pub const fn get(self) -> u32 {
        self.0.get() - 1
    }
}

impl fmt::Debug for NameIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NameIndex({})", self.get())
    }
}

/// A numeral's value, as its [`Chunk`](crate::Chunk)'s version reads it, and
/// its spelling.
///
/// A float is never negative or NaN, since no numeral spells one, and is
/// infinite only for a decimal past the largest double. An integer exists
/// only where Lua 5.3+ reads one, and is negative only where hex digits wrap.
/// `ContentEq` compares the value alone, so `16` and `0x10` hold the same
/// tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct NumberLiteral<'ast> {
    pub value: LuaNumber,
    /// The numeral as written, or `None` for one built in code, which a
    /// printer spells from the value.
    #[content_eq(skip)]
    pub raw: Option<ThinStr<'ast>>,
    pub span: Span,
    /// Whether a numeral spelled as an integer reads as that integer, as
    /// `luck_token::Numeral::is_exact` says.
    #[content_eq(skip)]
    pub is_exact: bool,
}

/// Luau: an exact 64-bit integer, `123i`, negative where its hex or binary
/// digits pass `i64::MAX`, and its spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct IntegerLiteral<'ast> {
    pub value: i64,
    /// The literal as written, or `None` for one built in code.
    #[content_eq(skip)]
    pub raw: Option<ThinStr<'ast>>,
    pub span: Span,
}

/// A string's value, bytes that need not be UTF-8, and its spelling.
///
/// A literal the parser read with nothing to decode borrows its value from
/// the source. `ContentEq` compares the value alone, so `"a"`, `'\97'`, and
/// `[[a]]` hold the same tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Ast)]
pub struct StringLiteral<'ast> {
    pub value: ThinBytes<'ast>,
    /// The literal as written, quotes or long brackets and escapes included,
    /// or for a piece of an interpolated string, what lies between its
    /// delimiters. Bytes, as the source is, which need not be UTF-8. `None`
    /// for one built in code, which a printer spells from the value.
    #[content_eq(skip)]
    pub raw: Option<ThinBytes<'ast>>,
    pub span: Span,
}

impl<'ast> StringLiteral<'ast> {
    /// The value as text, when it's UTF-8.
    pub fn as_str(&self) -> Option<&'ast str> {
        str::from_utf8(self.value.as_bytes()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_saturates_below_u32_max() {
        let last = u32::MAX - 1;
        for (index, kept) in [(0, 0), (7, 7), (last, last), (u32::MAX, last)] {
            assert_eq!(NameIndex::new(index).get(), kept, "{index}");
        }
        assert_eq!(format!("{:?}", NameIndex::new(3)), "NameIndex(3)");
    }

    #[test]
    fn equality_and_debug_leave_a_name_s_index_out() {
        let name = Name::new("x".into(), None, Span::new(0, 1));
        let numbered = Name { index: Some(NameIndex::new(0)), ..name };
        assert_eq!(name, numbered);
        assert_eq!(
            format!("{numbered:?}"),
            r#"Name { text: "x", span: Span { start: 0, end: 1 } }"#
        );
    }
}
