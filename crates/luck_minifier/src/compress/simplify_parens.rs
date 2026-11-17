//! Dropping parentheses that change neither grouping nor how many values an
//! expression yields.

use luck_allocator::Allocator;
use luck_ast::{
    Expression, FunctionArgs, Suffix, SuffixedExpression, is_multi_value, is_prefix_expr,
};
use luck_token::{BinOp, OperandSide};

use crate::expr::{
    binds_looser_than_unary, can_unwrap_in_binop, ends_in_if_expr, inner_operand_side,
};

/// Strips the parentheses around `expr`, its operands, or a unary
/// operator's operand, wherever printing them bare reparses as the same
/// tree. Says whether it did.
pub(super) fn strip<'ast>(allocator: &'ast Allocator, expr: &mut Expression<'ast>) -> bool {
    match expr {
        Expression::Parenthesized(paren) if can_remove_parens(&paren.expr) => {
            *expr = paren.expr.take();
            true
        }
        Expression::BinaryOp(binary_op) => {
            let op = binary_op.links.first().op;
            let inner_side = inner_operand_side(binary_op);
            let run = &mut **binary_op;
            let mut stripped = strip_operand(&mut run.first, op, OperandSide::Left);
            if let Some((last, inner)) = run.links.split_last_mut() {
                for link in inner {
                    stripped = stripped.max(strip_operand(&mut link.operand, op, inner_side));
                }
                stripped = stripped.max(strip_operand(&mut last.operand, op, OperandSide::Right));
            }
            // A run the parens held at this level now prints as part of it.
            if stripped == Stripped::RunAtLevel {
                run.splice_operands(allocator);
            }
            stripped != Stripped::Nothing
        }
        Expression::UnaryOp(unop) => {
            if let Expression::Parenthesized(paren) = &mut unop.operand
                && can_unwrap_in_unary(&paren.expr)
            {
                unop.operand = paren.expr.take();
                return true;
            }
            false
        }
        Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stripped {
    Nothing,
    Operand,
    /// The parens held a run at the operator's own level, which the run
    /// around it has to splice.
    RunAtLevel,
}

fn strip_operand(operand: &mut Expression<'_>, op: BinOp, side: OperandSide) -> Stripped {
    let Expression::Parenthesized(paren) = operand else {
        return Stripped::Nothing;
    };
    if !can_unwrap_in_binop(&paren.expr, op, side) {
        return Stripped::Nothing;
    }
    *operand = paren.expr.take();
    if matches!(operand, Expression::BinaryOp(run) if run.precedence().0 == op.precedence().0) {
        Stripped::RunAtLevel
    } else {
        Stripped::Operand
    }
}

/// Strips a chain's parenthesized prefix wherever what it holds can stand
/// bare there, and says whether it did.
///
/// That's a name, a chain, or nested parentheses, and not a literal, as in
/// `("s"):rep(2)`. In front of a Luau instantiation the parentheses also
/// stay around a call or `...`, whose values they truncate.
pub(super) fn strip_prefix<'ast>(
    allocator: &'ast Allocator,
    chain: &mut SuffixedExpression<'ast>,
) -> bool {
    let Expression::Parenthesized(paren) = &mut chain.prefix else {
        return false;
    };
    let is_instantiated = matches!(chain.suffixes.first(), Suffix::TypeInstantiation { .. });
    let can_strip =
        is_prefix_expr(&paren.expr) && (!is_instantiated || can_remove_parens(&paren.expr));
    if !can_strip {
        return false;
    }
    chain.prefix = paren.expr.take();
    chain.splice_prefix(allocator);
    true
}

/// Strips parentheses where nothing groups with what they hold, and says
/// whether it did.
///
/// That's anywhere but an operand or a chain's prefix. They stay only around
/// a call or `...`, whose values they truncate.
pub(super) fn strip_standalone(expr: &mut Expression<'_>) -> bool {
    let Expression::Parenthesized(paren) = expr else {
        return false;
    };
    if is_multi_value(&paren.expr) || matches!(paren.expr, Expression::Error(_)) {
        return false;
    }
    *expr = paren.expr.take();
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
    if let Expression::StringLiteral(boxed) = arg {
        let literal = **boxed;
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

fn can_remove_parens(inner: &Expression<'_>) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::Parenthesized(_)
        | Expression::FunctionDef(_) => true,
        // Calls and `...` aren't safe: dropping the parens restores multiple
        // values.
        Expression::Suffixed(chain) => !chain.is_call(),
        Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::VarArg(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

fn can_unwrap_in_unary(inner: &Expression<'_>) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Name(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_) => true,
        Expression::Suffixed(chain) => chain.is_var(),
        Expression::UnaryOp(_) => !ends_in_if_expr(inner),
        // Not safe: `-(a+b)` isn't `-a+b`.
        Expression::BinaryOp(binary_op) => {
            !binds_looser_than_unary(binary_op) && !ends_in_if_expr(inner)
        }
        Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use luck_allocator::Allocator;
    use luck_ast::{LastStatement, expr_nesting_depth};
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, compressed, minify_with};
    use crate::transform_config::TransformConfig;

    const PARENS: TransformConfig = TransformConfig { simplify_parens: true, ..NO_PASSES };

    fn assert_outputs(version: LuaVersion, cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            assert_eq!(minify_with(source, version, &PARENS), expected, "source: {source}");
        }
    }

    #[test]
    fn strips_what_grouping_and_value_counts_allow() {
        assert_outputs(
            LuaVersion::Lua54,
            &[
                ("local x = (42)", "local x=42"),
                ("local x = (y)", "local x=y"),
                ("return (a * b) + c", "return a*b+c"),
                ("return (x) * 3", "return x*3"),
                ("return a .. (b .. c)", "return a..b..c"),
                ("print(\"hello\")", "print\"hello\""),
                ("foo({1, 2})", "foo{1,2}"),
                ("return -(x)", "return-x"),
                ("return not (a.b)", "return not a.b"),
            ],
        );
    }

    #[test]
    fn strips_parens_where_nothing_groups_with_them() {
        assert_outputs(
            LuaVersion::Lua54,
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
                // A call or `...` keeps the parens that truncate its values.
                ("return (f()), (...)", "return(f()),(...)"),
            ],
        );
        assert_outputs(
            LuaVersion::Luau,
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
            LuaVersion::Lua54,
            &[
                ("return (x).y", "return x.y"),
                ("(f)()", "f()"),
                ("return (a.b).c", "return a.b.c"),
                ("return (f()).x", "return f().x"),
                ("return ((x)):m()", "return x:m()"),
                ("f(); (g)()", "f();g()"),
            ],
        );
    }

    #[test]
    fn keeps_what_grouping_and_value_counts_need() {
        assert_outputs(
            LuaVersion::Lua54,
            &[
                // Parens around a call truncate its values.
                ("local x = (foo())", "local x=(foo())"),
                ("return (f())", "return(f())"),
                ("return (f()) + 1", "return(f())+1"),
                ("return a * (b + c)", "return a*(b+c)"),
                ("return (1 + 2) * 3", "return(1+2)*3"),
                ("return (-a) ^ b", "return(-a)^b"),
                // Concat is right-associative, so (a .. b) .. c isn't a .. b .. c.
                ("return (a .. b) .. c", "return(a..b)..c"),
                ("return -(a + b)", "return-(a+b)"),
                ("return (\"s\"):rep(2)", "return(\"s\"):rep(2)"),
            ],
        );
    }

    #[test]
    fn a_run_freed_of_its_parens_joins_the_run_around_it() {
        let allocator = Allocator::new();
        let source = "return (a + b) - c, x .. (y .. z), (a * b) + c";
        let block = compressed(&allocator, source, PARENS);
        let Some(LastStatement::Return(return_stmt)) = &block.last_stmt else {
            panic!("expected a return");
        };
        let depths: Vec<usize> = return_stmt.exprs.iter().map(expr_nesting_depth).collect();
        // `a * b` binds tighter than `+`, so it stays a run of its own.
        assert_eq!(depths, [1, 1, 2]);
    }

    #[test]
    fn keeps_parens_on_an_instantiated_literal_in_every_position() {
        assert_outputs(
            LuaVersion::Luau,
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
            LuaVersion::Luau,
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
