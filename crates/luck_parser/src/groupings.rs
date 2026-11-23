use luck_token::Span;

/// The parentheses the source wrote around expressions only to group them,
/// which the tree leaves out, by the expression each pair holds.
///
/// A formatter that keeps the ones a user wrote, a lint about them, and an
/// editor's expanding selection find them here.
#[derive(Debug, Default)]
pub struct Groupings(Vec<Grouping>);

#[derive(Debug, Clone, Copy)]
struct Grouping {
    expr: Span,
    /// From the outermost `(` around `expr` to its `)`.
    parens: Span,
}

impl Groupings {
    /// The span of the parentheses around the expression at `expr`, from the
    /// outermost `(` to its `)`, if the source grouped it.
    #[must_use]
    pub fn around(&self, expr: Span) -> Option<Span> {
        let key = (expr.start, expr.end);
        let index = self.0.binary_search_by_key(&key, Grouping::key).ok()?;
        self.0.get(index).map(|grouping| grouping.parens)
    }

    /// Records `parens` around `expr`. A pair around one just recorded is
    /// the outermost of the two.
    pub(crate) fn record(&mut self, expr: Span, parens: Span) {
        if let Some(last) = self.0.last_mut()
            && last.expr == expr
        {
            last.parens = parens;
        } else {
            self.0.push(Grouping { expr, parens });
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.0.len()
    }

    /// [`Self::around`] among the ones recorded from `from` on, before they're
    /// sorted.
    pub(crate) fn around_since(&self, from: usize, expr: Span) -> Option<Span> {
        let recorded = self.0.get(from..).unwrap_or_default();
        recorded.iter().find(|grouping| grouping.expr == expr).map(|grouping| grouping.parens)
    }

    /// Parentheses close inside out, so they're recorded out of order.
    pub(crate) fn sort(&mut self) {
        self.0.sort_unstable_by_key(Grouping::key);
    }
}

impl Grouping {
    const fn key(&self) -> (u32, u32) {
        (self.expr.start, self.expr.end)
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::{LuaVersion, index_to_offset};

    use super::*;

    #[test]
    fn finds_the_outermost_pair_around_each_grouped_expression() {
        let allocator = Allocator::new();
        let source = "return ((a)) + (b * (c))";
        let parse = crate::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let at = |text: &str| {
            let start = source.find(text).expect("text is in the source");
            Span::new(index_to_offset(start), index_to_offset(start.saturating_add(text.len())))
        };
        assert_eq!(parse.groupings.around(at("a")), Some(at("((a))")));
        assert_eq!(parse.groupings.around(at("b * (c)")), Some(at("(b * (c))")));
        assert_eq!(parse.groupings.around(at("c")), Some(at("(c)")));
        assert_eq!(parse.groupings.around(at("b")), None);
    }

    #[test]
    fn keeps_no_record_of_parentheses_the_tree_keeps() {
        let allocator = Allocator::new();
        let parse = crate::parse(&allocator, b"return (f()), (...)", LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        assert_eq!(parse.groupings.len(), 0);
    }
}
