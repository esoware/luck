use luck_ast::{
    Block, Expression, FunctionArgs, FunctionCall, Transform, Var, is_prefix_expression,
};

use crate::expr::{can_unwrap_in_binop_lhs, can_unwrap_in_binop_rhs, parenthesize};

/// Removes parentheses that operator precedence and the operand's expression
/// kind already make redundant.
pub(crate) fn simplify(block: Block) -> Block {
    ParenSimplifier.transform_block(block)
}

struct ParenSimplifier;

impl ParenSimplifier {
    /// Keeps a prefix's own parens, which a literal like `("str"):method()`
    /// needs, but strips any nested inside them.
    fn transform_prefix(&mut self, prefix: Expression) -> Expression {
        match prefix {
            Expression::Parenthesized(paren) => {
                let transformed = self.transform_expression(paren.expr);
                parenthesize(transformed)
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::TypeInstantiation(_)
            | Expression::Error(_)) => self.transform_expression(other),
        }
    }
}

impl Transform for ParenSimplifier {
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        match self.walk_expression(expr) {
            Expression::Parenthesized(paren) => {
                if can_remove_parens(&paren.expr) {
                    paren.expr
                } else {
                    parenthesize(paren.expr)
                }
            }
            Expression::BinaryOp(mut binop) => {
                if let Expression::Parenthesized(paren) = binop.left {
                    if can_unwrap_in_binop_lhs(&paren.expr, binop.op) {
                        binop.left = paren.expr;
                    } else {
                        binop.left = parenthesize(paren.expr);
                    }
                }
                if let Expression::Parenthesized(paren) = binop.right {
                    if can_unwrap_in_binop_rhs(&paren.expr, binop.op) {
                        binop.right = paren.expr;
                    } else {
                        binop.right = parenthesize(paren.expr);
                    }
                }
                Expression::BinaryOp(binop)
            }
            Expression::UnaryOp(mut unop) => {
                if let Expression::Parenthesized(paren) = unop.operand {
                    if can_unwrap_in_unary(&paren.expr) {
                        unop.operand = paren.expr;
                    } else {
                        unop.operand = parenthesize(paren.expr);
                    }
                }
                Expression::UnaryOp(unop)
            }
            // Luau: the walk stripped these parens, but `{}<<T>>` doesn't
            // parse, so anything that isn't a valid prefix gets them back.
            Expression::TypeInstantiation(mut instantiation) => {
                if !is_prefix_expression(&instantiation.expr) {
                    instantiation.expr = parenthesize(instantiation.expr);
                }
                Expression::TypeInstantiation(instantiation)
            }
            other @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Var(_)
            | Expression::FunctionCall(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_)) => other,
        }
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
                        | Expression::Var(_)
                        | Expression::FunctionCall(_)
                        | Expression::Parenthesized(_)
                        | Expression::BinaryOp(_)
                        | Expression::UnaryOp(_)
                        | Expression::IfExpression(_)
                        | Expression::InterpolatedString(_)
                        | Expression::TypeCast(_)
                        | Expression::TypeInstantiation(_)
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

    fn transform_var(&mut self, var: Var) -> Var {
        match var {
            Var::FieldAccess(mut field_access) => {
                field_access.prefix = self.transform_prefix(field_access.prefix);
                Var::FieldAccess(field_access)
            }
            Var::Index(mut index_expr) => {
                index_expr.prefix = self.transform_prefix(index_expr.prefix);
                index_expr.index = self.transform_expression(index_expr.index);
                Var::Index(index_expr)
            }
            name @ Var::Name(_) => self.walk_var(name),
        }
    }

    fn transform_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        call.callee = self.transform_prefix(call.callee);
        call.explicit_type_args = call.explicit_type_args.map(|mut type_args| {
            *type_args = self.walk_type_args(*type_args);
            type_args
        });
        call.args = self.transform_function_args(call.args);
        call
    }
}

const fn can_remove_parens(inner: &Expression) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::Parenthesized(_)
        | Expression::FunctionDef(_) => true,
        // Calls and `...` aren't safe: dropping the parens restores multiple
        // values.
        Expression::FunctionCall(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::VarArg(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

const fn can_unwrap_in_unary(inner: &Expression) -> bool {
    match inner {
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Var(_)
        | Expression::TableConstructor(_)
        | Expression::FunctionDef(_)
        | Expression::UnaryOp(_) => true,
        // Not safe: `-(a+b)` isn't `-a+b`.
        Expression::BinaryOp(_)
        | Expression::FunctionCall(_)
        | Expression::VarArg(_)
        | Expression::Parenthesized(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
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
