use luck_ast::{
    Block, Expression, FunctionArgs, Suffix, SuffixedExpression, Transform, is_prefix_expr,
};
use luck_token::{BinOp, OperandSide};

use crate::expr::{
    binds_looser_than_unary, can_unwrap_in_binop_lhs, can_unwrap_in_binop_rhs, ends_in_if_expr,
    inner_operand_side, parenthesize,
};

pub(crate) fn simplify(block: Block) -> Block {
    ParenSimplifier.transform_block(block)
}

struct ParenSimplifier;

// Out of line, so an expression nested below holds only the walk's frame,
// not these temporaries.
fn strip_parens(walked: Expression) -> Expression {
    match walked {
        Expression::Parenthesized(paren) => {
            let inner = paren.expr;
            if can_remove_parens(&inner) { inner } else { parenthesize(inner) }
        }
        Expression::BinaryOp(mut binary_op) => {
            let op = binary_op.links.first().op;
            let inner_side = inner_operand_side(&binary_op);
            strip_operand_parens(&mut binary_op.first, op, OperandSide::Left);
            if let Some((last, inner)) = binary_op.links.split_last_mut() {
                for link in inner {
                    strip_operand_parens(&mut link.operand, op, inner_side);
                }
                strip_operand_parens(&mut last.operand, op, OperandSide::Right);
            }
            // A run the parens held at this level now prints as part of it.
            binary_op.splice_operands();
            Expression::BinaryOp(binary_op)
        }
        Expression::UnaryOp(mut unop) => {
            if let Expression::Parenthesized(paren) = &mut unop.operand {
                let inner = paren.expr.take();
                unop.operand =
                    if can_unwrap_in_unary(&inner) { inner } else { parenthesize(inner) };
            }
            Expression::UnaryOp(unop)
        }
        other @ (Expression::Nil(_)
        | Expression::False(_)
        | Expression::True(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::Error(_)) => other,
    }
}

fn strip_operand_parens(operand: &mut Expression, op: BinOp, side: OperandSide) {
    if let Expression::Parenthesized(paren) = operand {
        let inner = paren.expr.take();
        let can_unwrap = match side {
            OperandSide::Left => can_unwrap_in_binop_lhs(&inner, op),
            OperandSide::Right => can_unwrap_in_binop_rhs(&inner, op),
        };
        *operand = if can_unwrap { inner } else { parenthesize(inner) };
    }
}

impl Transform for ParenSimplifier {
    fn transform_expr(&mut self, expr: Expression) -> Expression {
        let walked = self.walk_expr(expr);
        strip_parens(walked)
    }

    // f("str") becomes f"str", and f({...}) becomes f{...}.
    fn transform_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        match args {
            FunctionArgs::Parenthesized { span, args: mut call_args } => {
                if call_args.len() == 1
                    && let Some(arg) = call_args.items.pop()
                {
                    match arg {
                        Expression::StringLiteral(literal) => {
                            return FunctionArgs::StringLiteral(literal);
                        }
                        Expression::TableConstructor(mut table) => {
                            *table = self.walk_table_constructor(*table);
                            return FunctionArgs::TableConstructor(table);
                        }
                        other @ (Expression::Nil(_)
                        | Expression::False(_)
                        | Expression::True(_)
                        | Expression::Number(_)
                        | Expression::Integer(_)
                        | Expression::VarArg(_)
                        | Expression::FunctionDef(_)
                        | Expression::Name(_)
                        | Expression::Suffixed(_)
                        | Expression::Parenthesized(_)
                        | Expression::BinaryOp(_)
                        | Expression::UnaryOp(_)
                        | Expression::IfExpression(_)
                        | Expression::InterpolatedString(_)
                        | Expression::TypeCast(_)
                        | Expression::Error(_)) => call_args.items.push(other),
                    }
                }
                FunctionArgs::Parenthesized { span, args: self.walk_punctuated_exprs(call_args) }
            }
            FunctionArgs::TableConstructor(mut table) => {
                *table = self.walk_table_constructor(*table);
                FunctionArgs::TableConstructor(table)
            }
            literal @ FunctionArgs::StringLiteral(_) => literal,
        }
    }

    /// A prefix keeps its own parens, which a literal like
    /// `("str"):method()` needs, but sheds any nested inside them. In front
    /// of a Luau instantiation they go too, unless they hold something
    /// `{}<<T>>` or `f()<<T>>` couldn't stand without.
    fn transform_suffixed(&mut self, mut chain: SuffixedExpression) -> SuffixedExpression {
        let is_instantiated = matches!(chain.suffixes.first(), Suffix::TypeInstantiation { .. });
        // Out of the walk, which would strip the prefix's own parens.
        let prefix = chain.prefix.take();
        let mut walked = self.walk_suffixed(chain);
        walked.prefix = if let Expression::Parenthesized(paren) = prefix {
            let inner = self.transform_expr(paren.expr);
            if is_instantiated && is_prefix_expr(&inner) && can_remove_parens(&inner) {
                inner
            } else {
                parenthesize(inner)
            }
        } else {
            self.transform_expr(prefix)
        };
        walked.splice_prefix();
        walked
    }
}

fn can_remove_parens(inner: &Expression) -> bool {
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

fn can_unwrap_in_unary(inner: &Expression) -> bool {
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
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::{assert_reparses, run_pass};

    fn apply(source: &str) -> String {
        run_pass(source, LuaVersion::Lua54, simplify)
    }

    #[test]
    fn removes_parens_around_number() {
        let result = apply("local x = (42)\n");
        assert!(!result.contains("(42)"), "Got: {result}");
    }

    #[test]
    fn removes_parens_around_variable() {
        let result = apply("local x = (y)\n");
        assert!(!result.contains("(y)"), "Got: {result}");
    }

    #[test]
    fn keeps_parens_around_call() {
        let result = apply("local x = (foo())\n");
        assert!(result.contains("(foo())"), "Multi-return parens removed: {result}");
    }

    #[test]
    fn higher_precedence_inner_unwraps() {
        let result = apply("return (a * b) + c\n");
        assert!(
            !result.contains("(a*b)") && !result.contains("(a * b)"),
            "Higher prec inner should unwrap, got: {result}"
        );
    }

    #[test]
    fn lower_precedence_rhs_keeps_parens() {
        let result = apply("return a * (b + c)\n");
        assert!(result.contains('('), "Lower prec rhs must keep parens, got: {result}");
    }

    #[test]
    fn keeps_parens_lower_precedence_in_multiply() {
        let result = apply("return (1 + 2) * 3\n");
        assert!(result.contains('('), "Lower prec (1+2) must keep parens in multiply: {result}");
    }

    #[test]
    fn removes_parens_around_variable_in_multiply() {
        let result = apply("return (x) * 3\n");
        assert!(!result.contains("(x)"), "Parens around variable should be removed: {result}");
    }

    #[test]
    fn keeps_parens_unary_in_exponentiation() {
        let result = apply("return (-a) ^ b\n");
        assert!(result.contains("(-a)"), "Parens required for (-a)^b, got: {result}");
    }

    #[test]
    fn strips_parens_on_single_arg_string_call() {
        let result = apply("print(\"hello\")\n");
        assert!(
            result.contains("print\"hello\""),
            "Single string arg should strip parens: {result}"
        );
    }

    #[test]
    fn strips_parens_on_single_arg_table_call() {
        let result = apply("foo({1, 2})\n");
        assert!(result.contains("foo{"), "Single table arg should strip parens: {result}");
    }

    #[test]
    fn keeps_parens_left_concat_in_concat() {
        // Concat is right-associative, so (a .. b) .. c is not a .. b .. c.
        let result = apply("return (a .. b) .. c\n");
        assert!(result.contains('('), "Left-associative concat grouping needs parens: {result}");
    }

    #[test]
    fn removes_parens_right_concat_in_concat() {
        // Concat is right-associative, so a .. (b .. c) is a .. b .. c.
        let result = apply("return a .. (b .. c)\n");
        assert!(!result.contains('('), "Right-associative concat parens are redundant: {result}");
    }

    #[test]
    fn keeps_parens_around_call_in_binop() {
        // `+` already truncates f() to one value, but the pass keeps parens
        // around every call.
        let result = apply("return (f()) + 1\n");
        assert!(result.contains("(f())"), "Parens around call in binop must be kept: {result}");
    }

    fn assert_luau_output(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            let result = run_pass(source, LuaVersion::Luau, simplify);
            assert_reparses(&result, LuaVersion::Luau);
            assert_eq!(result, expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_parens_on_instantiated_literal() {
        assert_luau_output(&[
            ("return ({})<<T>>\n", "return({})<<T>>"),
            ("return (\"s\")<<T>>\n", "return(\"s\")<<T>>"),
            ("return (1)<<T>>\n", "return(1)<<T>>"),
            ("return (1i)<<T>>\n", "return(1i)<<T>>"),
            ("return (nil)<<T>>\n", "return(nil)<<T>>"),
            ("return (true)<<T>>\n", "return(true)<<T>>"),
            ("return (false)<<T>>\n", "return(false)<<T>>"),
            ("return (function() end)<<T>>\n", "return(function()end)<<T>>"),
            ("return (({}))<<T>>\n", "return({})<<T>>"),
        ]);
    }

    #[test]
    fn keeps_parens_on_instantiated_literal_in_every_position() {
        assert_luau_output(&[
            ("return ({})<<T>><<U>>\n", "return({})<<T>><<U>>"),
            ("return ({})<<T>>()\n", "return({})<<T>>()"),
            ("return (\"s\")<<T>>.x\n", "return(\"s\")<<T>>.x"),
            ("return ({})<<T>>[1]\n", "return({})<<T>>[1]"),
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
        ]);
    }

    #[test]
    fn strips_parens_on_instantiated_var() {
        assert_luau_output(&[
            ("return (x)<<T>>\n", "return x<<T>>"),
            ("return (x.y)<<T>>\n", "return x.y<<T>>"),
            ("return (x[1])<<T>>\n", "return x[1]<<T>>"),
            ("return ((x))<<T>>\n", "return x<<T>>"),
            ("return (x)<<T>>()\n", "return x<<T>>()"),
            ("return (x)<<T>>.y\n", "return x<<T>>.y"),
        ]);
    }

    #[test]
    fn keeps_parens_on_instantiated_multi_value() {
        // The parens truncate to one value, as in (f())<<T>>.
        assert_luau_output(&[
            ("return (f())<<T>>\n", "return(f())<<T>>"),
            ("return (...)<<T>>\n", "return(...)<<T>>"),
        ]);
    }
}
