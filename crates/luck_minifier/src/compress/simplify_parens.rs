//! Dropping parentheses that change nothing.
//!
//! The tree keeps only those that truncate a call or `...` to one value or
//! stop a `-` before an integer literal from reading as a negative one,
//! and the printer writes whatever grouping needs. So what's left to drop
//! is a truncation wherever only the first value counts anyway, the
//! parentheses around a lone string or table argument, and the ones a run
//! held in a run at its level prints with where it groups the same without.

use luck_allocator::Allocator;
use luck_ast::{Expression, FunctionArgs, Slot, Suffix, SuffixedExpression};

/// Where an expression the walk leaves stands, as far as its parentheses
/// go.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    /// Where every value it yields may count: the last of a list that takes
    /// more values than it has items, as a `return` or a call's arguments
    /// does, or a type, which the walk doesn't read.
    AllValues,
    /// Where only its first value counts: an operand, a condition, a key, a
    /// field, an index, or any but the last of a list.
    OneValue,
    /// The operand of a `-`, under which an integer literal reads as a
    /// negative one.
    Negated,
}

/// Strips the parentheses around `expr` where they change nothing at
/// `place`, and says whether it did.
pub(super) fn strip(expr: &mut Expression<'_>, place: Place) -> bool {
    let Expression::Parenthesized(paren) = expr else {
        return false;
    };
    let is_kept = match place {
        Place::AllValues => paren.expr.is_multi_value(),
        Place::OneValue => false,
        Place::Negated => matches!(paren.expr, Expression::Integer(_)),
    };
    if is_kept || matches!(paren.expr, Expression::Error(_)) {
        return false;
    }
    *expr = paren.expr.take();
    true
}

/// Strips a chain's parenthesized prefix where what it holds stands bare
/// there, which a chain does, and says whether it did.
///
/// In front of an instantiation the parentheses stay around a call,
/// whose values they truncate.
pub(super) fn strip_prefix<'ast>(
    allocator: &'ast Allocator,
    chain: &mut SuffixedExpression<'ast>,
) -> bool {
    let Expression::Parenthesized(paren) = &mut chain.prefix else {
        return false;
    };
    let is_instantiated = matches!(chain.suffixes.first(), Suffix::TypeInstantiation { .. });
    if paren.expr.needs_parens(Slot::PREFIX) || (is_instantiated && paren.expr.is_multi_value()) {
        return false;
    }
    chain.prefix = paren.expr.take();
    chain.splice_prefix(allocator);
    true
}

/// `f("str")` becomes `f"str"`, and `f({...})` becomes `f{...}`. Says
/// whether it rewrote the arguments.
pub(super) fn strip_call_args(args: &mut FunctionArgs<'_>) -> bool {
    let FunctionArgs::Parenthesized { args: exprs, .. } = args else {
        return false;
    };
    let [arg] = &mut **exprs else {
        return false;
    };
    if matches!(arg, Expression::StringLiteral(_))
        && let Expression::StringLiteral(literal) = arg.take()
    {
        *args = FunctionArgs::StringLiteral(literal);
        return true;
    }
    if matches!(arg, Expression::TableConstructor(_))
        && let Expression::TableConstructor(table) = arg.take()
    {
        *args = FunctionArgs::TableConstructor(table);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::{Statement, expr_nesting_depth};

    use crate::fixtures::{NO_PASSES, assert_outputs, compressed, minify_with};
    use crate::transform_config::TransformConfig;

    const PARENS: TransformConfig = TransformConfig { simplify_parens: true, ..NO_PASSES };

    fn apply(source: &str) -> String {
        minify_with(source, &PARENS)
    }

    #[test]
    fn strips_what_grouping_and_value_counts_allow() {
        assert_outputs(
            apply,
            &[
                ("local x = (42)", "local x=42"),
                ("local x = (y)", "local x=y"),
                ("return (a * b) + c", "return a*b+c"),
                ("return (x) * 3", "return x*3"),
                ("return a .. (b .. c)", "return a..b..c"),
                ("return (a + b) - c", "return a+b-c"),
                ("print(\"hello\")", "print\"hello\""),
                ("foo({1, 2})", "foo{1,2}"),
                ("return -(x)", "return-x"),
                ("return not (a.b)", "return not a.b"),
                ("return (f()) + 1, -(...), #(f())", "return f()+1,-...,#f()"),
            ],
        );
    }

    #[test]
    fn strips_a_truncation_wherever_one_value_counts() {
        assert_outputs(
            apply,
            &[
                ("if (f()) then g() end", "if f()then g()end"),
                ("while (f()) do end repeat until (...)", "while f()do end repeat until..."),
                (
                    "return {(f()), x}, {x, (f())}, {(f()), k = 1}",
                    "return{f(),x},{x,(f())},{f(),k=1}",
                ),
                ("return t[(f())], {k = (f()), [(f())] = 1}", "return t[f()],{k=f(),[f()]=1}"),
                ("local a = (f()) local b, c = (f())", "local a=f()local b,c=(f())"),
                ("a, b = (f()), (g()) c, d = (h())", "a,b=f(),g()c,d=(h())"),
                ("g((f()), (...)) return (f()), (...)", "g(f(),(...))return f(),(...)"),
                (
                    "for i = (f()), (g()) do end for k in (f()) do end",
                    "for i=f(),g()do end for k in(f())do end",
                ),
            ],
        );
        assert_outputs(
            apply,
            &[
                ("x += (f()) return `{(f())}`", "x+=f()return`{f()}`"),
                ("return if (f()) then (g()) else (...)", "return if f()then g()else..."),
            ],
        );
    }

    #[test]
    fn strips_parens_where_nothing_groups_with_them() {
        assert_outputs(
            apply,
            &[
                ("local x = (a + b)", "local x=a+b"),
                ("if (a or b) then f() end", "if a or b then f()end"),
                ("while (not a) do f() end", "while not a do f()end"),
                ("return (a or b), (c)", "return a or b,c"),
                ("f((a + b), (c))", "f(a+b,c)"),
                ("f((\"s\"))", "f\"s\""),
                ("return {(a + b), k = (c), [(d)] = (e)}", "return{a+b,k=c,[d]=e}"),
                ("return t[(a + b)]", "return t[a+b]"),
                ("return ((a + b))", "return a+b"),
                // A call or `...` keeps the parens that truncate its values
                // where every value would count.
                ("return (f()), (...)", "return f(),(...)"),
            ],
        );
        assert_outputs(
            apply,
            &[
                ("local x = (a :: T)", "local x=a::T"),
                ("return if (a) then (b) else (c or d)", "return if a then b else c or d"),
                ("return `{(a + b)}`", "return`{a+b}`"),
            ],
        );
    }

    #[test]
    fn strips_parens_a_chain_prefix_stands_without() {
        assert_outputs(
            apply,
            &[
                ("return (x).y", "return x.y"),
                ("(f)()", "f()"),
                ("return (a.b).c", "return a.b.c"),
                ("return (f()).x", "return f().x"),
                ("return ((x)):m()", "return x:m()"),
                ("f(); (g)()", "f()g()"),
            ],
        );
    }

    #[test]
    fn keeps_what_grouping_and_value_counts_need() {
        assert_outputs(
            apply,
            &[
                // Parens around a call truncate its values.
                ("local x, y = (foo())", "local x,y=(foo())"),
                ("return (f())", "return(f())"),
                ("return a * (b + c)", "return a*(b+c)"),
                ("return (1 + 2) * 3", "return(1+2)*3"),
                ("return (-a) ^ b", "return(-a)^b"),
                ("return a - (b - c)", "return a-(b-c)"),
                // Concat is right-associative, so (a .. b) .. c isn't a .. b .. c.
                ("return (a .. b) .. c", "return(a..b)..c"),
                ("return -(a + b)", "return-(a+b)"),
                ("return (\"s\"):rep(2)", "return(\"s\"):rep(2)"),
            ],
        );
        // `-(1i)` negates an integer, which raises, and `-1i` is a literal.
        assert_outputs(
            apply,
            &[("return -(1i), not (1i), (1i) + x, (1i) :: T", "return-(1i),not 1i,1i+x,1i::T")],
        );
    }

    #[test]
    fn a_run_freed_of_its_parens_joins_the_run_around_it() {
        let allocator = Allocator::new();
        let source = "return (a + b) - c, x .. (y .. z), (a * b) + c";
        let chunk = compressed(&allocator, source, PARENS);
        let [Statement::Return(return_stmt)] = &*chunk.block.stmts else {
            panic!("expected a return");
        };
        let depths: Vec<usize> = return_stmt.exprs.iter().map(expr_nesting_depth).collect();
        // `a * b` binds tighter than `+`, so it stays a run of its own.
        assert_eq!(depths, [1, 1, 2]);
    }

    #[test]
    fn keeps_parens_on_an_instantiated_literal_in_every_position() {
        assert_outputs(
            apply,
            &[
                ("return ({})<<T>>\n", "return({})<<T>>"),
                ("return (\"s\")<<T>>\n", "return(\"s\")<<T>>"),
                ("return (1)<<T>>\n", "return(1)<<T>>"),
                ("return (1i)<<T>>\n", "return(1i)<<T>>"),
                ("return (nil)<<T>>\n", "return(nil)<<T>>"),
                ("return (function() end)<<T>>\n", "return(function()end)<<T>>"),
                ("return (({}))<<T>>\n", "return({})<<T>>"),
                ("return ({})<<T>><<U>>\n", "return({})<<T>><<U>>"),
                ("return ({})<<T>>()\n", "return({})<<T>>()"),
                ("return (\"s\")<<T>>.x\n", "return(\"s\")<<T>>.x"),
                ("return ({})<<T>>:m<<U>>()\n", "return({})<<T>>:m<<U>>()"),
                ("({})<<T>>()\n", "({})<<T>>()"),
                ("f();({})<<T>>()\n", "f();({})<<T>>()"),
                ("({})<<T>>.x = 1\n", "({})<<T>>.x=1"),
                ("({})<<T>>[1] += 1\n", "({})<<T>>[1]+=1"),
                ("print(({})<<T>>)\n", "print(({})<<T>>)"),
                ("return {a = ({})<<T>>}\n", "return{a=({})<<T>>}"),
                ("return ({})<<T>> + 1\n", "return({})<<T>>+1"),
                ("return -({})<<T>>\n", "return-({})<<T>>"),
                ("return ({})<<T>> :: U\n", "return({})<<T>>::U"),
                ("return if c then ({})<<T>> else 1\n", "return if c then({})<<T>>else 1"),
                ("return `x{({})<<T>>}y`\n", "return`x{({})<<T>>}y`"),
                ("type X = typeof(({})<<T>>)\n", "type X=typeof(({})<<T>>)"),
                // The parens truncate to one value, as in (f())<<T>>.
                ("return (f())<<T>>\n", "return(f())<<T>>"),
                ("return (...)<<T>>\n", "return(...)<<T>>"),
            ],
        );
    }

    #[test]
    fn strips_parens_on_an_instantiated_var() {
        assert_outputs(
            apply,
            &[
                ("return (x)<<T>>\n", "return x<<T>>"),
                ("return (x.y)<<T>>\n", "return x.y<<T>>"),
                ("return (x[1])<<T>>\n", "return x[1]<<T>>"),
                ("return ((x))<<T>>\n", "return x<<T>>"),
                ("return (x)<<T>>()\n", "return x<<T>>()"),
                ("return (x)<<T>>.y\n", "return x<<T>>.y"),
            ],
        );
    }
}
