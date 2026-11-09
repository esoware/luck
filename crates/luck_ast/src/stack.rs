//! Stack safety for chains, whose depth the parser's nesting cap can't bound.
//!
//! `a.b.c...`, `f()()...`, `1 + 1 + ...`, and `T???...` grow in loops, and
//! inlining can stack parens and operators without limit. Every recursion
//! along such a chain goes through [`ensure_sufficient_stack`], and everything
//! else is bounded by that cap.

use core::fmt;
use core::mem;

use luck_token::Span;

use crate::expr::{
    BinaryOp, Expression, FieldAccess, FunctionArgs, FunctionCall, IndexExpression,
    ParenthesizedExpression, TypeCast, TypeInstantiation, UnaryOp, Var,
};
use crate::punctuated::Punctuated;
use crate::types::{OptionalType, Type};

/// Stack kept free below a chain link. It covers the nesting the parser
/// allows beneath one link, so only links need a guard.
const RED_ZONE: usize = 1024 * 1024;

const STACK_SEGMENT: usize = 8 * 1024 * 1024;

/// Runs `descend` with enough stack to recurse, moving onto a fresh
/// heap-allocated segment first when the current one is nearly spent.
///
/// Wrap every recursive step along a chain link in this: a `BinaryOp` operand,
/// a call's callee, a `Var::Index` or `Var::FieldAccess` prefix, a unary
/// operand, a parenthesized expression, a type cast's or instantiation's
/// expression, and `OptionalType::type_value`. Don't wrap recursion the
/// parser's nesting cap already bounds, such as nested tables or statements.
#[inline]
pub fn ensure_sufficient_stack<R, Descend>(descend: Descend) -> R
where
    Descend: FnOnce() -> R,
{
    stacker::maybe_grow(RED_ZONE, STACK_SEGMENT, descend)
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "`take` lives beside the `Drop` impls that force it"
)]
impl Expression {
    /// Chain links implement `Drop`, so their children can't be moved out
    /// directly. Assign the replacement back, or let the link drop.
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "`take` lives beside the `Drop` impls that force it"
)]
impl FunctionArgs {
    /// Moves the value out and leaves an empty list behind, for the same
    /// reason as [`Expression::take`].
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Parenthesized { span: Span::default(), args: Punctuated::empty() })
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "`take` lives beside the `Drop` impls that force it"
)]
impl Type {
    /// Moves the value out and leaves a childless placeholder behind, for
    /// the same reason as [`Expression::take`].
    #[must_use]
    pub fn take(&mut self) -> Self {
        mem::replace(self, Self::Error(Span::default()))
    }
}

const fn is_chain_link(expr: &Expression) -> bool {
    match expr {
        Expression::Var(Var::Index(_) | Var::FieldAccess(_))
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_) => true,
        Expression::Var(Var::Name(_))
        | Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => false,
    }
}

/// Follows `link`'s chain in a loop, so drop glue only ever sees a link whose
/// chain child is a placeholder.
fn drop_chain(mut link: Expression) {
    loop {
        link = match &mut link {
            Expression::BinaryOp(binary_op) => binary_op.lhs.take(),
            Expression::UnaryOp(unary_op) => unary_op.operand.take(),
            Expression::Parenthesized(parenthesized) => parenthesized.expr.take(),
            Expression::FunctionCall(call) => call.callee.take(),
            Expression::Var(Var::FieldAccess(field_access)) => field_access.prefix.take(),
            Expression::Var(Var::Index(index)) => index.prefix.take(),
            // Luau
            Expression::TypeCast(cast) => cast.expr.take(),
            // Luau
            Expression::TypeInstantiation(instantiation) => instantiation.expr.take(),
            Expression::Var(Var::Name(_))
            | Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => return,
        };
    }
}

// `$branch` is a right operand or an index, which only inlining can grow past
// the parser's cap, so it recurses behind the guard.
macro_rules! drop_expression_chain {
    ($node:ident: $chain:ident $(, $branch:ident)?) => {
        impl Drop for $node {
            fn drop(&mut self) {
                drop_chain(self.$chain.take());
                $(
                    if is_chain_link(&self.$branch) {
                        ensure_sufficient_stack(|| drop(self.$branch.take()));
                    }
                )?
            }
        }
    };
}

drop_expression_chain!(BinaryOp: lhs, rhs);
drop_expression_chain!(UnaryOp: operand);
drop_expression_chain!(ParenthesizedExpression: expr);
drop_expression_chain!(FunctionCall: callee);
drop_expression_chain!(IndexExpression: prefix, index);
drop_expression_chain!(FieldAccess: prefix);
drop_expression_chain!(TypeCast: expr);
drop_expression_chain!(TypeInstantiation: expr);

impl Drop for OptionalType {
    fn drop(&mut self) {
        let mut next = self.type_value.take();
        while let Type::Optional(mut link) = next {
            next = link.type_value.take();
        }
    }
}

/// Clone, equality, and debug formatting by hand, each behind the guard,
/// because the derived impls follow the chain field by field.
macro_rules! guarded_chain_traits {
    ($($node:ident { $($field:ident),+ $(,)? })+) => {$(
        impl Clone for $node {
            fn clone(&self) -> Self {
                ensure_sufficient_stack(|| Self { $($field: self.$field.clone()),+ })
            }
        }

        impl PartialEq for $node {
            fn eq(&self, other: &Self) -> bool {
                ensure_sufficient_stack(|| true $(&& self.$field == other.$field)+)
            }
        }

        impl fmt::Debug for $node {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                ensure_sufficient_stack(|| {
                    f
                        .debug_struct(stringify!($node))
                        $(.field(stringify!($field), &self.$field))+
                        .finish()
                })
            }
        }
    )+};
}

guarded_chain_traits! {
    BinaryOp { span, lhs, op, rhs }
    UnaryOp { span, op, operand }
    ParenthesizedExpression { span, expr }
    FunctionCall { span, callee, args, method, explicit_type_args }
    IndexExpression { span, prefix, index }
    FieldAccess { span, prefix, name }
    TypeCast { span, expr, type_annotation }
    TypeInstantiation { span, expr, type_args }
    OptionalType { span, type_value }
}
