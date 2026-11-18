use luck_allocator::{Box, List, ThinStr};
use luck_token::{BinOp, CompoundOp, UnOp};

use crate::one_or_more::OneOrMore;

/// Whether two values hold the same tree, however each was written down.
///
/// It leaves out spans, a name's [`NameIndex`](crate::NameIndex), and the
/// fields that record only how the source spelled a node, such as a table's
/// trailing separator, so `a==1` and `a == 1` hold the same tree where `==`
/// would tell them apart. A literal compares by its text as written, so
/// `"x"` and `'x'` differ.
///
/// `luck_ast_tools` generates it for every type that derives `Ast`. Ported
/// from `oxc_span`'s `ContentEq`.
pub trait ContentEq {
    #[must_use]
    fn content_eq(&self, other: &Self) -> bool;
}

impl<T> ContentEq for &T
where
    T: ContentEq + ?Sized,
{
    fn content_eq(&self, other: &Self) -> bool {
        (**self).content_eq(*other)
    }
}

impl ContentEq for str {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for ThinStr<'_> {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for bool {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for u32 {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for BinOp {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for UnOp {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl ContentEq for CompoundOp {
    fn content_eq(&self, other: &Self) -> bool {
        self == other
    }
}

impl<T> ContentEq for Option<T>
where
    T: ContentEq,
{
    fn content_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(lhs), Some(rhs)) => lhs.content_eq(rhs),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl<T> ContentEq for Box<'_, T>
where
    T: ContentEq,
{
    fn content_eq(&self, other: &Self) -> bool {
        (**self).content_eq(&**other)
    }
}

impl<T> ContentEq for [T]
where
    T: ContentEq,
{
    fn content_eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(lhs, rhs)| lhs.content_eq(rhs))
    }
}

impl<T> ContentEq for List<'_, T>
where
    T: ContentEq,
{
    fn content_eq(&self, other: &Self) -> bool {
        (**self).content_eq(&**other)
    }
}

impl<T> ContentEq for OneOrMore<'_, T>
where
    T: ContentEq,
{
    fn content_eq(&self, other: &Self) -> bool {
        (**self).content_eq(&**other)
    }
}
