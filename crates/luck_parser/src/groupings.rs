use luck_token::Span;

/// The parentheses the source wrote around expressions only to group them,
/// which the tree leaves out, by the expression each pair holds, in source
/// order.
///
/// A formatter that keeps the ones a user wrote, a lint about them, and an
/// editor's expanding selection find them here.
#[derive(Debug, Default)]
pub struct Groupings(Vec<Grouping>);

#[derive(Debug, Clone, Copy)]
struct Grouping {
    expr: Span,
    /// From the outermost `(` around `expr` to its `)`, or empty for a pair
    /// the tree kept, or one still open.
    parens: Span,
}

impl Groupings {
    /// The span of the parentheses around the expression at `expr`, from the
    /// outermost `(` to its `)`, if the source grouped it.
    pub fn around(&self, expr: Span) -> Option<Span> {
        let index = self.0.binary_search_by_key(&key(expr), Grouping::key).ok()?;
        Some(self.0[index].parens)
    }

    /// [`Self::around`], searched for from `cursor`, where the search before
    /// left it, so a printer asking in source order takes a step or two.
    #[inline]
    pub fn around_from(&self, expr: Span, cursor: &mut usize) -> Option<Span> {
        let wanted = key(expr);
        let mut index = (*cursor).min(self.0.len());
        while self.0.get(index).is_some_and(|grouping| grouping.key() < wanted) {
            index += 1;
        }
        let before = &self.0[..index];
        if before.last().is_some_and(|grouping| grouping.key() >= wanted) {
            index = before.partition_point(|grouping| grouping.key() < wanted);
        }
        *cursor = index;
        self.0
            .get(index)
            .filter(|grouping| grouping.key() == wanted)
            .map(|grouping| grouping.parens)
    }

    /// Opens an entry for parentheses whose `(` is the next token, which
    /// [`Self::record`] fills when their `)` closes, so entries fall in
    /// source order.
    pub(crate) fn open(&mut self) -> usize {
        let slot = self.0.len();
        self.0.push(Grouping { expr: Span::new(0, 0), parens: Span::new(0, 0) });
        slot
    }

    /// Records the parentheses `slot` opened around `expr`.
    pub(crate) fn record(&mut self, slot: usize, expr: Span, parens: Span) {
        self.0[slot] = Grouping { expr, parens };
    }

    pub(crate) const fn len(&self) -> usize {
        self.0.len()
    }

    /// [`Self::around`] among the ones opened from `from` on, while the
    /// parse goes on.
    pub(crate) fn around_since(&self, from: usize, expr: Span) -> Option<Span> {
        self.0[from..]
            .iter()
            .find(|grouping| grouping.expr == expr && !grouping.parens.is_empty())
            .map(|grouping| grouping.parens)
    }

    /// Drops the entries of pairs the tree kept, and of all but the outermost
    /// pair around one expression, which comes first. A parse that recovered
    /// from an error can leave them out of order, which a sort settles.
    pub(crate) fn finish(&mut self) {
        self.0.retain(|grouping| !grouping.parens.is_empty());
        self.0.dedup_by_key(|grouping| grouping.key());
        if !self.0.is_sorted_by_key(Grouping::key) {
            self.0.sort_by_key(Grouping::key);
        }
    }
}

impl Grouping {
    const fn key(&self) -> (u32, u32) {
        (self.expr.start, self.expr.end)
    }
}

const fn key(expr: Span) -> (u32, u32) {
    (expr.start, expr.end)
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
            Span::new(index_to_offset(start), index_to_offset(start + text.len()))
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

    #[test]
    fn finds_each_pair_from_any_cursor() {
        let allocator = Allocator::new();
        let source = "return (a), ((b)), (c) + (d), (e .. (f)), g, (h)";
        let parse = crate::parse(&allocator, source.as_bytes(), LuaVersion::Lua54);
        assert!(parse.errors.is_empty(), "{:?}", parse.errors);
        let groupings = &parse.groupings;
        let queries: Vec<Span> = (0..index_to_offset(source.len()))
            .flat_map(|start| {
                (start..=index_to_offset(source.len())).map(move |end| Span::new(start, end))
            })
            .collect();
        for start_cursor in 0..=groupings.len() + 2 {
            let mut cursor = start_cursor;
            for &expr in queries.iter().chain(queries.iter().rev()) {
                assert_eq!(groupings.around_from(expr, &mut cursor), groupings.around(expr));
            }
        }
    }
}
