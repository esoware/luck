//! Where a printer writes an expression decides whether it needs parentheses
//! there, so a tree holds none of its own for grouping, as `oxc_codegen`
//! derives them from precedence.

use luck_token::{BinOp, UnOp};

use crate::expr::{BinaryOp, Expression};
use crate::types::Type;

/// What a printer writes right after an expression, as far as reading the
/// output back goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follower {
    /// Nothing an expression could take in: the end, a closer, a separator,
    /// or a keyword.
    Nothing,
    /// A binary operator.
    Operator(BinOp),
    /// Luau: `::`, which follows only a simple expression.
    Cast,
    /// A field, an index, a call, or a Luau instantiation, which follows only
    /// a name, a chain, or parentheses.
    Suffix,
}

/// Where a printer writes an expression: the operator it's an operand of,
/// and what follows it.
///
/// A printer that writes parentheses around an expression writes what's in
/// them at [`Slot::FREE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// The precedence of the operator the expression is an operand of, or 0
    /// for none.
    level: u8,
    follower: Follower,
}

impl Slot {
    /// Where nothing groups with the expression: a list item, a condition, a
    /// key, a value, or anything inside a closer.
    pub const FREE: Self = Self { level: 0, follower: Follower::Nothing };
    /// A chain's prefix, before its first suffix.
    pub const PREFIX: Self = Self { level: 0, follower: Follower::Suffix };
    /// Luau: a cast's operand, before its `::`.
    pub const CAST_OPERAND: Self = Self { level: 0, follower: Follower::Cast };

    /// The operand of `run` at `index`, its first at 0, where the run is in
    /// this slot: each is followed by the next operator, and the last by what
    /// follows the run.
    #[must_use]
    pub fn operand(self, run: &BinaryOp<'_>, index: usize) -> Self {
        let follower =
            run.links.get(index).map_or(self.follower, |link| Follower::Operator(link.op));
        Self { level: run.precedence().0, follower }
    }

    /// The operand of a unary operator in this slot.
    #[must_use]
    pub const fn unary_operand(self) -> Self {
        Self { level: UnOp::PRECEDENCE, follower: self.follower }
    }
}

impl Expression<'_> {
    /// Whether this expression, printed bare in `slot`, would read back as
    /// something else.
    ///
    /// A run is the operators read in a row, so one written as another's
    /// operand at its own level reads back as part of it, and takes
    /// parentheses even where its operators would group the same. A
    /// [`Expression::Parenthesized`] prints its own.
    #[must_use]
    pub fn needs_parens(&self, slot: Slot) -> bool {
        match self {
            Self::Name(_) | Self::Suffixed(_) | Self::Parenthesized(_) | Self::Error(_) => false,
            Self::Nil(_)
            | Self::False(_)
            | Self::True(_)
            | Self::Number(_)
            | Self::Integer(_)
            | Self::StringLiteral(_)
            | Self::VarArg(_)
            | Self::FunctionDef(_)
            | Self::TableConstructor(_)
            | Self::InterpolatedString(_) => slot.follower == Follower::Suffix,
            Self::BinaryOp(run) => {
                run.precedence().0 <= slot.level
                    || matches!(slot.follower, Follower::Cast | Follower::Suffix)
            }
            // The operand reads on through `^`, the one operator that binds
            // tighter, and through a cast.
            Self::UnaryOp(_) => match slot.follower {
                Follower::Operator(op) => op.precedence().0 > UnOp::PRECEDENCE,
                Follower::Cast | Follower::Suffix => true,
                Follower::Nothing => false,
            },
            // Luau: the `else` branch reads on through whatever follows.
            Self::IfExpression(_) => slot.follower != Follower::Nothing,
            // Luau: a type that ends in a bare name reads on through `<` as
            // its generic arguments, any type through `|` and `&` as a union
            // or an intersection, and Luau rejects a cast of a cast.
            Self::TypeCast(cast) => match slot.follower {
                Follower::Operator(BinOp::Lt) => ends_in_bare_name(&cast.ty),
                Follower::Operator(op) => matches!(op, BinOp::BitOr | BinOp::BitAnd),
                Follower::Cast | Follower::Suffix => true,
                Follower::Nothing => false,
            },
        }
    }
}

/// Whether `ty` prints a name with no generic arguments last.
fn ends_in_bare_name(ty: &Type<'_>) -> bool {
    let mut current = ty;
    loop {
        current = match current {
            Type::Name(_) => return true,
            Type::Named(named) => return named.generics.is_none(),
            Type::Function(function) => &function.return_type,
            Type::Union(union) => match union.types.last() {
                Some(last) => last,
                None => return false,
            },
            Type::Intersection(intersection) => match intersection.types.last() {
                Some(last) => last,
                None => return false,
            },
            Type::Variadic(variadic) => &variadic.ty,
            Type::Typeof(_)
            | Type::Table(_)
            | Type::Optional(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::GenericPack(_)
            | Type::Error(_) => return false,
        };
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_token::Span;

    use super::*;
    use crate::expr::BinaryLink;
    use crate::fixtures::Fixture;
    use crate::one_or_more::OneOrMore;

    fn run<'ast>(
        first: Expression<'ast>,
        op: BinOp,
        operand: Expression<'ast>,
        allocator: &'ast Allocator,
    ) -> Expression<'ast> {
        let links = OneOrMore::new(BinaryLink::new(op, operand), allocator);
        Expression::new_binary_op(Span::default(), first, links, allocator)
    }

    fn operand_slots(expr: &Expression<'_>, outer: Slot) -> Vec<Slot> {
        let Expression::BinaryOp(binary_op) = expr else {
            panic!("expected a binary run, got {expr:?}");
        };
        (0..=binary_op.links.len()).map(|index| outer.operand(binary_op, index)).collect()
    }

    #[test]
    fn a_run_takes_parentheses_under_an_operator_as_tight_or_tighter() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let sum = || run(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"), allocator);
        let product = run(synth.name_expr("x"), BinOp::Mul, synth.name_expr("y"), allocator);
        let [multiplicand, multiplier] = *operand_slots(&product, Slot::FREE) else {
            panic!("expected two operands");
        };
        assert!(sum().needs_parens(multiplicand), "`(a + b) * y`");
        assert!(sum().needs_parens(multiplier), "`x * (a + b)`");
        let total = run(synth.name_expr("x"), BinOp::Add, synth.name_expr("y"), allocator);
        let [augend, addend] = *operand_slots(&total, Slot::FREE) else {
            panic!("expected two operands");
        };
        assert!(sum().needs_parens(augend), "a run at its own level reads back merged");
        assert!(sum().needs_parens(addend));
        assert!(!product.needs_parens(augend), "`x * y + y`");
        assert!(!sum().needs_parens(Slot::FREE));
        assert!(sum().needs_parens(Slot::FREE.unary_operand()), "`-(a + b)`");
        let power = run(synth.name_expr("a"), BinOp::Pow, synth.name_expr("b"), allocator);
        assert!(!power.needs_parens(Slot::FREE.unary_operand()), "`-a ^ b`");
        assert!(sum().needs_parens(Slot::PREFIX));
        assert!(sum().needs_parens(Slot::CAST_OPERAND));
    }

    #[test]
    fn a_unary_operator_takes_parentheses_before_what_its_operand_reads_on_through() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let negation = synth.unary_op(UnOp::Neg, synth.name_expr("a"));
        let power = run(synth.name_expr("x"), BinOp::Pow, synth.name_expr("y"), allocator);
        let [base, exponent] = *operand_slots(&power, Slot::FREE) else {
            panic!("expected two operands");
        };
        assert!(negation.needs_parens(base), "`(-a) ^ y`");
        assert!(!negation.needs_parens(exponent), "`x ^ -a`");
        let product = run(synth.name_expr("x"), BinOp::Mul, synth.name_expr("y"), allocator);
        assert!(!negation.needs_parens(operand_slots(&product, Slot::FREE)[0]), "`-a * y`");
        assert!(negation.needs_parens(Slot::CAST_OPERAND), "`(-a) :: T`");
        assert!(negation.needs_parens(Slot::PREFIX));
    }

    #[test]
    fn an_if_expression_takes_parentheses_before_anything() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let if_expr = synth.if_expr(synth.name_expr("c"), synth.nil(), [], synth.nil());
        let sum = run(synth.name_expr("x"), BinOp::Add, synth.name_expr("y"), allocator);
        let [first, last] = *operand_slots(&sum, Slot::FREE) else {
            panic!("expected two operands");
        };
        assert!(if_expr.needs_parens(first));
        assert!(!if_expr.needs_parens(last), "`x + if c then nil else nil` ends where it reads");
        let [_, followed] = *operand_slots(&sum, first) else {
            panic!("expected two operands");
        };
        assert!(if_expr.needs_parens(followed), "the run's own follower reaches its last operand");
        assert!(!if_expr.needs_parens(Slot::FREE.unary_operand()));
    }

    #[test]
    fn a_cast_takes_parentheses_before_what_a_type_reads_on_through() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let allocator = fixture.allocator();
        let cast = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
        for (op, is_wrapped) in [(BinOp::Lt, true), (BinOp::Gt, false), (BinOp::Add, false)] {
            let comparison = run(synth.name_expr("x"), op, synth.name_expr("y"), allocator);
            let [first, last] = *operand_slots(&comparison, Slot::FREE) else {
                panic!("expected two operands");
            };
            assert_eq!(cast.needs_parens(first), is_wrapped, "{op:?}");
            assert!(!cast.needs_parens(last), "{op:?}");
        }
        assert!(cast.needs_parens(Slot::CAST_OPERAND), "`(a :: T) :: U`");
        assert!(!cast.needs_parens(Slot::FREE.unary_operand()), "`-a :: T`");
        let comparison = run(synth.name_expr("x"), BinOp::Lt, synth.name_expr("y"), allocator);
        let before_less = operand_slots(&comparison, Slot::FREE)[0];
        let generic = synth.ty_generic("T", [synth.ty_named("U")]);
        for (ty, is_wrapped) in [
            (generic, false),
            (synth.ty_optional(synth.ty_named("T")), false),
            (synth.ty_union([synth.ty_named("T"), synth.ty_named("U")]), true),
        ] {
            let typed = synth.type_cast(synth.name_expr("a"), ty);
            assert_eq!(typed.needs_parens(before_less), is_wrapped, "{typed:?}");
        }
    }

    #[test]
    fn only_a_prefix_expression_stands_before_a_suffix() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        for bare in [synth.name_expr("a"), synth.field(synth.name_expr("a"), "b")] {
            assert!(!bare.needs_parens(Slot::PREFIX), "{bare:?}");
        }
        assert!(!synth.single_value(synth.vararg()).needs_parens(Slot::PREFIX));
        for wrapped in [synth.string("s"), synth.table([]), synth.number_i64(1), synth.vararg()] {
            assert!(wrapped.needs_parens(Slot::PREFIX), "{wrapped:?}");
            assert!(!wrapped.needs_parens(Slot::CAST_OPERAND), "{wrapped:?}");
        }
    }
}
