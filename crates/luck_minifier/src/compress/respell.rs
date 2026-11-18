//! What `shorten_strings` and `shorten_numbers` share.

use luck_allocator::Allocator;
use luck_ast::Literal;

/// Respells literals in a scratch buffer it keeps from one to the next.
#[derive(Default)]
pub(super) struct Respeller {
    scratch: String,
}

impl Respeller {
    /// Respells `literal` with `respell`, and says whether that's shorter.
    pub(super) fn shorten<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        literal: &mut Literal<'ast>,
        respell: impl FnOnce(&str, &mut String) -> bool,
    ) -> bool {
        self.scratch.clear();
        if !respell(literal.text, &mut self.scratch) || self.scratch.len() >= literal.text.len() {
            return false;
        }
        literal.text = allocator.alloc_str(&self.scratch);
        true
    }
}
