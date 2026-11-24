use std::mem;

use luck_allocator::{Allocator, Box};
use luck_token::Associativity;

use crate::expr::{BinaryOp, Expression};

impl<'ast> BinaryOp<'ast> {
    /// What every operator in the run shares.
    pub fn precedence(&self) -> (u8, Associativity) {
        self.links.first().op.precedence()
    }

    /// Joins in the operand that's a run at this level where it groups as
    /// this run would without it: the first of a left-associative run, or
    /// the last of a right-associative one. Says whether it joined any.
    ///
    /// The printers write such an operand in parentheses, so this drops them,
    /// and the tree nests no deeper than reading it back without them would.
    /// One run deep or any number, it's one pass over the links that move.
    pub fn splice_operands(&mut self, allocator: &'ast Allocator) -> bool {
        let (level, associativity) = self.precedence();
        let is_spliced = |operand: &Expression<'_>| matches!(operand, Expression::BinaryOp(inner) if inner.precedence().0 == level);
        match associativity {
            Associativity::Left => {
                if !is_spliced(&self.first) {
                    return false;
                }
                // Each run's links follow those of the run it starts with,
                // and wait here outermost first.
                let mut later = Vec::new();
                let mut first = self.first.take();
                while is_spliced(&first)
                    && let Expression::BinaryOp(inner) = first
                {
                    let Self { first: inner_first, links, .. } = Box::unbox(inner);
                    later.push(mem::replace(&mut self.links, links));
                    first = inner_first;
                }
                self.first = first;
                for links in later.into_iter().rev() {
                    for link in links {
                        self.links.push(link, allocator);
                    }
                }
            }
            Associativity::Right => {
                if !is_spliced(&self.links.last().operand) {
                    return false;
                }
                while is_spliced(&self.links.last().operand)
                    && let Expression::BinaryOp(inner) = self.links.last_mut().operand.take()
                {
                    let Self { first, links, .. } = Box::unbox(inner);
                    self.links.last_mut().operand = first;
                    for link in links {
                        self.links.push(link, allocator);
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use luck_token::BinOp;

    use super::*;
    use crate::fixtures::Fixture;

    fn ops(expr: &Expression<'_>) -> Vec<BinOp> {
        let Expression::BinaryOp(run) = expr else {
            panic!("expected a binary run, got {expr:?}");
        };
        run.links.iter().map(|link| link.op).collect()
    }

    #[test]
    fn splice_operands_joins_only_an_operand_that_groups_as_the_run_would() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let name = |name: &str| synth.name_expr(name);
        // (a + b) - (c * d + e) is a + b - (c * d + e).
        let left = fixture.unspliced_run(name("a"), BinOp::Add, name("b"));
        let product = fixture.unspliced_run(name("c"), BinOp::Mul, name("d"));
        let right = fixture.unspliced_run(product, BinOp::Add, name("e"));
        let Expression::BinaryOp(mut run) = fixture.unspliced_run(left, BinOp::Sub, right) else {
            panic!("expected a binary run");
        };
        assert!(run.splice_operands(allocator));
        assert_eq!(
            run.links.iter().map(|link| link.op).collect::<Vec<_>>(),
            [BinOp::Add, BinOp::Sub]
        );
        assert!(matches!(run.first, Expression::Name(_)));
        assert_eq!(ops(&run.links[1].operand), [BinOp::Add], "`c * d + e` keeps its grouping");
        assert!(!run.splice_operands(allocator));

        // a .. (b .. c) is a .. b .. c, but (a .. b) .. c isn't.
        let tail = fixture.unspliced_run(name("b"), BinOp::Concat, name("c"));
        let Expression::BinaryOp(mut concat) =
            fixture.unspliced_run(name("a"), BinOp::Concat, tail)
        else {
            panic!("expected a binary run");
        };
        assert!(concat.splice_operands(allocator));
        assert_eq!(concat.links.len(), 2);
        let head = fixture.unspliced_run(name("a"), BinOp::Concat, name("b"));
        let Expression::BinaryOp(mut grouped) =
            fixture.unspliced_run(head, BinOp::Concat, name("c"))
        else {
            panic!("expected a binary run");
        };
        assert!(!grouped.splice_operands(allocator));
        assert_eq!(ops(&grouped.first), [BinOp::Concat]);
    }

    #[test]
    fn splice_operands_joins_runs_nested_any_number_of_levels_deep() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let name = || synth.name_expr("x");
        let mut sum = name();
        let mut concat = name();
        for _ in 0..50_000 {
            sum = fixture.unspliced_run(sum, BinOp::Add, name());
            concat = fixture.unspliced_run(name(), BinOp::Concat, concat);
        }
        for nested in [sum, concat] {
            let Expression::BinaryOp(mut run) = nested else {
                panic!("expected a binary run");
            };
            run.splice_operands(fixture.allocator());
            assert_eq!(run.links.len(), 50_000);
            assert!(matches!(run.first, Expression::Name(_)));
            assert!(run.links.iter().all(|link| matches!(link.operand, Expression::Name(_))));
        }
    }
}
