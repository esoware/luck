//! How deep the tree being built nests, counted toward
//! [`MAX_NESTING_DEPTH`] as [`luck_ast::block_nesting_depth`] measures it.

use std::mem;

use luck_ast::MAX_NESTING_DEPTH;

use super::Parser;

impl Parser<'_> {
    /// Count one more level of nesting, or at [`MAX_NESTING_DEPTH`], stop
    /// the parse there and return false, leaving the count unchanged.
    #[inline]
    pub(super) fn enter_depth(&mut self) -> bool {
        // A host-stack budget, independent of target VM compiler limits.
        if self.nesting_depth >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.nesting_depth += 1;
        self.deepest_level = self.deepest_level.max(self.nesting_depth);
        true
    }

    /// Count one more pair of parentheses open, or at [`MAX_NESTING_DEPTH`]
    /// of them, stop the parse as [`Self::enter_depth`] does.
    ///
    /// The tree leaves them out, so they have a cap of their own: a tree
    /// within its cap prints with at most a pair at each of its levels, and
    /// reads back.
    #[inline]
    pub(super) fn enter_grouping(&mut self) -> bool {
        if self.grouping_depth >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.grouping_depth += 1;
        true
    }

    #[inline]
    pub(super) const fn exit_depth(&mut self) {
        self.nesting_depth -= 1;
    }

    /// The cap is fatal: one error at the current token, and the rest of the
    /// source is skipped, so every construct still open ends at the end of
    /// the source, where recovery reports nothing more.
    #[cold]
    #[inline(never)]
    fn stop_at_depth_limit(&mut self) {
        let span = self.token.span;
        self.syntax_error(
            span,
            format!(
                "maximum nesting depth exceeded (luck parser resource limit: {MAX_NESTING_DEPTH})"
            ),
        );
        while !self.is_at_eof() {
            self.advance();
        }
    }

    /// Start measuring an operand at the current level, returning the
    /// deepest level of the operand around it. Hand that to
    /// [`Self::end_operand`] once the operand is whole.
    #[inline]
    pub(super) const fn begin_operand(&mut self) -> usize {
        mem::replace(&mut self.deepest_level, self.nesting_depth)
    }

    /// Fold the finished operand's depth into that of the operand around it.
    #[inline]
    pub(super) fn end_operand(&mut self, outer_deepest_level: usize) {
        self.deepest_level = self.deepest_level.max(outer_deepest_level);
    }

    /// Count a node that wraps the operand parsed so far at `level`: a run of
    /// binary operators or suffixes, a cast, parentheses the tree keeps, or a
    /// type operator. Everything in the operand moves one level down, so the
    /// cap applies to its deepest level, and stops the parse as
    /// [`Self::enter_depth`] does.
    ///
    /// On success the parser stands one level in, where the wrapper's other
    /// children go, and the caller steps back to `level` after them.
    #[inline]
    pub(super) fn enter_wrapper(&mut self, level: usize) -> bool {
        if self.deepest_level >= MAX_NESTING_DEPTH {
            self.stop_at_depth_limit();
            return false;
        }
        self.deepest_level += 1;
        self.nesting_depth = level + 1;
        true
    }
}
