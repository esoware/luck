//! What `shorten_strings` and `shorten_numbers` share.

use luck_allocator::Allocator;
use luck_ast::Literal;
use rustc_hash::FxHashSet;

/// Respells literals, and remembers the ones it found as short as they get,
/// so a later walk skips them.
#[derive(Default)]
pub(super) struct LiteralCache {
    /// Each settled literal's text by where it lives and how long it is,
    /// which no other text shares, since nothing in the arena or the source
    /// moves or is freed while the tree lives.
    settled: FxHashSet<(usize, usize)>,
    scratch: String,
}

impl LiteralCache {
    /// Respells `literal` with `respell`, and says whether it did.
    pub(super) fn shorten<'ast>(
        &mut self,
        allocator: &'ast Allocator,
        literal: &mut Literal<'ast>,
        respell: impl FnOnce(&str, &mut String) -> bool,
    ) -> bool {
        let key = (literal.text.as_ptr().addr(), literal.text.len());
        if self.settled.contains(&key) {
            return false;
        }
        self.scratch.clear();
        if !respell(literal.text, &mut self.scratch) || self.scratch.len() >= literal.text.len() {
            self.settled.insert(key);
            return false;
        }
        literal.text = allocator.alloc_str(&self.scratch);
        // Every respelling is already as short as its value gets.
        self.settled.insert((literal.text.as_ptr().addr(), literal.text.len()));
        true
    }
}
