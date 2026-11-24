//! Luau: stripping every type before anything else reads the tree, as
//! `oxc_transformer` strips TypeScript's before `oxc_minifier` runs.
//!
//! Annotations, generic lists, return types, instantiations, and `type`
//! declarations go. A cast leaves what it holds, in parentheses when that's
//! a call or `...`, since a cast takes one value.

use luck_allocator::Allocator;
use luck_ast::{
    AttributedName, Block, Expression, FunctionBody, GetSpan as _, MethodCall, NumericFor,
    Parameter, Statement, Suffix, SuffixedExpression, Type, VarArgParam, VisitMut, is_multi_value,
};

pub(crate) fn strip_types<'ast>(allocator: &'ast Allocator, block: &mut Block<'ast>) {
    TypeStripper { allocator }.visit_block(block);
}

struct TypeStripper<'ast> {
    allocator: &'ast Allocator,
}

const fn is_instantiation(suffix: &Suffix<'_>) -> bool {
    matches!(suffix, Suffix::TypeInstantiation { .. })
}

impl<'ast> VisitMut<'ast> for TypeStripper<'ast> {
    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.walk_block(block);
        let mut kept = 0;
        for index in 0..block.stmts.len() {
            if !matches!(block.stmts[index], Statement::TypeDeclaration(_)) {
                block.stmts.swap(kept, index);
                kept += 1;
            }
        }
        block.stmts.truncate(kept);
    }

    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        self.walk_expr(expr);
        match expr {
            Expression::TypeCast(cast) => {
                let inner = cast.expr.take();
                *expr = if is_multi_value(&inner) {
                    let span = inner.span();
                    Expression::new_parenthesized(span, inner, self.allocator)
                } else {
                    inner
                };
            }
            Expression::Suffixed(chain) if chain.suffixes.iter().all(is_instantiation) => {
                let mut prefix = chain.prefix.take();
                if let Expression::Parenthesized(parenthesized) = &mut prefix
                    && !is_multi_value(&parenthesized.expr)
                {
                    prefix = parenthesized.expr.take();
                }
                *expr = prefix;
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
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => {}
        }
    }

    /// A chain made only of instantiations leaves its prefix in its place,
    /// which [`Self::visit_expr`] does.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &mut SuffixedExpression<'ast>) {
        self.walk_suffixed_expr(suffixed_expr);
        let mut kept = 0;
        for index in 0..suffixed_expr.suffixes.len() {
            if !is_instantiation(&suffixed_expr.suffixes[index]) {
                suffixed_expr.suffixes.swap(kept, index);
                kept += 1;
            }
        }
        if kept > 0 {
            suffixed_expr.suffixes.truncate(kept);
        }
        suffixed_expr.splice_prefix(self.allocator);
    }

    fn visit_method_call(&mut self, method_call: &mut MethodCall<'ast>) {
        method_call.type_args = None;
        self.walk_method_call(method_call);
    }

    fn visit_function_body(&mut self, function_body: &mut FunctionBody<'ast>) {
        function_body.generics = None;
        function_body.return_type = None;
        self.walk_function_body(function_body);
    }

    fn visit_param(&mut self, param: &mut Parameter<'ast>) {
        param.type_annotation = None;
    }

    fn visit_var_arg_param(&mut self, var_arg_param: &mut VarArgParam<'ast>) {
        var_arg_param.type_annotation = None;
    }

    fn visit_attributed_name(&mut self, attributed_name: &mut AttributedName<'ast>) {
        attributed_name.type_annotation = None;
    }

    fn visit_numeric_for(&mut self, numeric_for: &mut NumericFor<'ast>) {
        numeric_for.type_annotation = None;
        self.walk_numeric_for(numeric_for);
    }

    fn visit_type(&mut self, _ty: &mut Type<'ast>) {}
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use crate::fixtures::{NO_PASSES, minify_with};
    use crate::transform_config::TransformConfig;

    const STRIPPING: TransformConfig = TransformConfig { strip_types: true, ..NO_PASSES };

    fn strip(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &STRIPPING)
    }

    #[test]
    fn strips_every_type() {
        for (source, expected) in [
            ("local x: number = 1", "local x=1"),
            ("local x: number", "local x"),
            (
                "local function f<T>(a: T, ...: T): T return a end",
                "local function f(a,...)return a end",
            ),
            ("for i: number = 1, 2 do end", "for i=1,2 do end"),
            ("for k: string, v: number in t do end", "for k,v in t do end"),
            ("type T = number export type U = T print(1)", "print(1)"),
            ("type function f(t) return t end", ""),
            ("return x :: number, f<<T>>(1), t:m<<T>>(2), g<<T>>", "return x,f(1),t:m(2),g"),
            ("return (t :: any).x, (t.y :: any).z", "return t.x,t.y.z"),
            ("(x :: any).y = 1", "x.y=1"),
        ] {
            assert_eq!(strip(source), expected, "source: {source}");
        }
    }

    #[test]
    fn keeps_the_truncation_a_cast_makes() {
        for (source, expected) in [
            ("return f() :: number", "return(f())"),
            ("return ... :: number", "return(...)"),
            ("return (f())<<T>>", "return(f())"),
            ("return ({})<<T>>", "return{}"),
        ] {
            assert_eq!(strip(source), expected, "source: {source}");
        }
    }

    #[test]
    fn leaves_types_on_when_asked() {
        let source = "local x:number=1::number";
        assert_eq!(minify_with(source, LuaVersion::Luau, &NO_PASSES), source);
    }
}
