use core::mem;

use luck_allocator::List;
use luck_token::Span;

use crate::expr::Expression;
use crate::stmt::{Block, Statement};
use crate::types::Type;

impl Block<'_> {
    /// Moves the value out of a `&mut` and leaves an empty block behind, as
    /// [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::new(Span::new(0, 0), List::new()))
    }
}

impl Statement<'_> {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }
}

impl Expression<'_> {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, which costs no allocation. Assign the replacement back.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }
}

impl Type<'_> {
    /// Moves the value out of a `&mut` and leaves an error placeholder
    /// behind, as [`Expression::take`] does.
    #[must_use]
    pub const fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::new(0, 0)))
    }
}
