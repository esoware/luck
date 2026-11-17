//! Inlining a local read once: its value, a literal expression that can't
//! raise, moves to the read, in parentheses only where it would regroup.

use luck_ast::{BinaryOp, Expression, MAX_NESTING_DEPTH, expr_nesting_depth};
use luck_token::{BinOp, MAX_ACTIVE_LOCALS, OperandSide};

use super::Compressor;
use crate::expr::{MAX_ADDED_DEPTH, binds_looser_than_unary, literal_kind, parenthesize};

/// An inlined local's initializer, waiting for the local's one read.
pub(super) struct Value<'ast> {
    expr: Expression<'ast>,
    /// The inlined locals it took in, its own included. Each is still in
    /// scope where its local is declared, so a chain past
    /// [`MAX_ACTIVE_LOCALS`] of them is longer than one function can hold,
    /// and it stops there.
    locals: usize,
}

#[derive(Clone, Copy)]
pub(super) enum OperandPosition {
    Left(BinOp),
    Right(BinOp),
    Unary,
    Cast,
}

impl OperandPosition {
    pub(super) const fn of_side(op: BinOp, side: OperandSide) -> Self {
        match side {
            OperandSide::Left => Self::Left(op),
            OperandSide::Right => Self::Right(op),
        }
    }
}

/// Grouping lives in `Parenthesized` nodes and codegen prints the tree flat,
/// so a value that binds looser than its position would regroup on reparse.
///
/// A value is a closed literal expression, so none prints a Luau
/// if-expression last.
fn keeps_grouping(value: &Expression<'_>, position: OperandPosition) -> bool {
    match value {
        Expression::BinaryOp(run) => match position {
            OperandPosition::Left(op) => run.can_nest_bare(op, OperandSide::Left),
            OperandPosition::Right(op) => run.can_nest_bare(op, OperandSide::Right),
            OperandPosition::Unary => !binds_looser_than_unary(run),
            OperandPosition::Cast => false,
        },
        Expression::UnaryOp(_) => match position {
            // Unary ops bind looser than ^, so (-a)^b needs its parens.
            OperandPosition::Left(op) => op != BinOp::Pow,
            OperandPosition::Right(_) | OperandPosition::Unary => true,
            OperandPosition::Cast => false,
        },
        // Luau: `::` takes a simple expression, and `x :: A :: B` doesn't
        // parse.
        Expression::TypeCast(_) => !matches!(position, OperandPosition::Cast),
        Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_)
        | Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Name(_)
        | Expression::Suffixed(_)
        | Expression::Parenthesized(_)
        | Expression::TableConstructor(_)
        | Expression::IfExpression(_)
        | Expression::InterpolatedString(_)
        | Expression::Error(_) => true,
    }
}

/// The operand `expr` prints last, which is what a following token lands on.
fn last_operand<'tree, 'ast>(expr: &'tree mut Expression<'ast>) -> &'tree mut Expression<'ast> {
    let mut current = expr;
    loop {
        current = match current {
            Expression::BinaryOp(binary_op) => &mut binary_op.links.last_mut().operand,
            Expression::UnaryOp(unop) => &mut unop.operand,
            Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => return current,
        };
    }
}

/// Whether a local's value moves to its one read, `read_depth` levels deep,
/// having taken in the values of `landed_locals` locals inlined into it.
fn can_inline(value: &Expression<'_>, landed_locals: usize, read_depth: usize) -> bool {
    if landed_locals >= MAX_ACTIVE_LOCALS || literal_kind(value).is_none() {
        return false;
    }
    let depth = expr_nesting_depth(value);
    // The value can gain parentheses where it lands, and a cast inside it
    // a second pair.
    depth <= MAX_ADDED_DEPTH
        && read_depth.saturating_add(2).saturating_add(depth) <= MAX_NESTING_DEPTH
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the compressor is one type with its rewrites split by flag"
)]
impl<'ast> Compressor<'ast> {
    /// Takes `value`, the initializer of the local `name`, to wait for the
    /// local's one read, if the scan found one, `inline_read_depth` levels
    /// deep, and the value can move there. Says whether it took it.
    pub(super) fn hold_value(
        &mut self,
        name: &str,
        value: &mut Expression<'ast>,
        landed_locals: usize,
        inline_read_depth: Option<usize>,
    ) -> bool {
        if let Some(read_depth) = inline_read_depth
            && can_inline(value, landed_locals, read_depth)
            && let Some(id) = self.names.find(name)
        {
            let locals = landed_locals.saturating_add(1);
            self.values.insert(id, Value { expr: value.take(), locals });
            return true;
        }
        false
    }

    /// An inlined local's value for its one read, which it reaches as the
    /// walk does, already rewritten.
    pub(super) fn take_value(&mut self, name: &str) -> Option<Expression<'ast>> {
        let value = self.values.remove(&self.names.find(name)?)?;
        self.landed_locals = self.landed_locals.saturating_add(value.locals);
        Some(value.expr)
    }

    /// Only in an operand position can a value regroup, so only there does
    /// it gain parens. Anywhere else it stays bare, since `return (1+2)` is
    /// longer than `return 1+2`.
    pub(super) fn visit_operand(
        &mut self,
        operand: &mut Expression<'ast>,
        position: OperandPosition,
    ) {
        if let Expression::Name(name) = operand
            && !self.values.is_empty()
            && let Some(value) = self.take_value(name.text)
        {
            if keeps_grouping(&value, position) {
                self.has_bare_operand = true;
                *operand = value;
            } else {
                *operand = parenthesize(self.allocator, value);
            }
            return;
        }
        self.visit_expr_in(operand, true);
    }

    /// Luau: a cast has no closing token and `T<` opens generic arguments, so
    /// a cast inlined just before `<` needs parens of its own. A cast that was
    /// already there parsed fine, so only inlining can cause this.
    pub(super) fn close_casts_before_less_than(&mut self, run: &mut BinaryOp<'ast>) {
        let allocator = self.allocator;
        let mut before = &mut run.first;
        for link in &mut *run.links {
            if link.op == BinOp::Lt
                && let Expression::Name(name) = last_operand(before)
                && let Some(id) = self.names.find(name.text)
                && let Some(value) = self.values.get_mut(&id)
            {
                let cast = last_operand(&mut value.expr);
                if matches!(cast, Expression::TypeCast(_)) {
                    *cast = parenthesize(allocator, cast.take());
                }
            }
            before = &mut link.operand;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write as _;

    use luck_allocator::Allocator;
    use luck_ast::{LastStatement, expr_nesting_depth};
    use luck_token::LuaVersion;

    use super::Compressor;
    use crate::fixtures::{NO_PASSES, assert_outputs, minify_with, parse, walk_once};
    use crate::transform_config::TransformConfig;

    const INLINING: TransformConfig = TransformConfig { inline_locals: true, ..NO_PASSES };

    fn inline(source: &str) -> String {
        minify_with(source, LuaVersion::Lua54, &INLINING)
    }

    fn inline_luau(source: &str) -> String {
        minify_with(source, LuaVersion::Luau, &INLINING)
    }

    #[test]
    fn inlines_single_use_literals() {
        assert_outputs(
            inline,
            &[
                ("local x = 42 return x", "return 42"),
                ("local x = 42 local y = x + 1 return y", "return 42+1"),
                (
                    "local function foo() local x = 42 return x end return foo",
                    "local function foo()return 42 end return foo",
                ),
                ("if true then local x = 42 print(x) end", "if true then print(42)end"),
                ("local x = 1 print(x)", "print(1)"),
                ("local x = 1 local function f() return x end", "local function f()return 1 end"),
                ("do local x = 1 do print(x) end end", "do do print(1)end end"),
                ("repeat local x = 1 until x == y", "repeat until 1==y"),
                ("repeat local x = 1 repeat until x until z", "repeat repeat until 1 until z"),
            ],
        );
    }

    #[test]
    fn keeps_what_inlining_would_change() {
        for source in [
            // More than one read.
            "local x=42 print(x)return x",
            // A call, a read, a table, a closure, and `...` each carry an
            // effect or an identity.
            "local x=foo()return x",
            "local x=y return x",
            "local t={1,2,3}return t",
            "local f=function()return 1 end return f",
            "local function f(...)local x=...return x end",
            // A read outside the declaration's scope is a global's.
            "print(x)local x=1",
            "do local x=1 end print(x)",
            // A write would land on a global.
            "local x=1 x=2",
            "local x=\"s\"function x.f()end print(x)",
            // The error must still fire before the print.
            "local x=1+nil print(\"before\")return x",
        ] {
            assert_eq!(inline(source), source);
        }
        assert_eq!(inline_luau("local x=1 x+=2"), "local x=1 x+=2");
        assert_eq!(inline_luau("local x=foo::Bar return x"), "local x=foo::Bar return x");
    }

    #[test]
    fn keeps_a_name_another_binding_wears() {
        assert_eq!(
            inline("local self = 5 function M:get() return self end"),
            "local self=5 function M:get()return self end"
        );
        assert_eq!(
            minify_with(
                "local arg = 5 local function f(...) return arg end return f",
                LuaVersion::Lua51,
                &INLINING
            ),
            "local arg=5 local function f(...)return arg end return f"
        );
        for source in [
            "local args=1 local function f(...args)return args end",
            "local x=1 global x print(x)",
            "local x=1 global function g()local x=2 return x end return g",
        ] {
            let result = minify_with(source, LuaVersion::Lua55, &INLINING);
            assert!(!result.contains("return 1"), "{source}: {result}");
        }
    }

    #[test]
    fn parenthesizes_a_value_that_would_regroup_where_it_lands() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 / 0 return 3 % s", "return 3%(1/0)"),
                ("local s = 1 + 2 return s * 2", "return(1+2)*2"),
                ("local s = 1 + 2 return 2 * s", "return 2*(1+2)"),
                ("local s = 1 - 2 return 5 - s", "return 5-(1-2)"),
                ("local s = 2 ^ 3 return s ^ 2", "return(2^3)^2"),
                ("local s = 1 .. 2 return s .. 3", "return(1 ..2)..3"),
                ("local s = 1 + 2 return 2 ^ s", "return 2^(1+2)"),
                ("local s = 1 or 2 return s and 3", "return(1 or 2)and 3"),
                ("local s = -2 return s ^ y", "return(-2)^y"),
                ("local s = not nil return s ^ y", "return(not nil)^y"),
                ("local s = 1 + 2 return -s", "return-(1+2)"),
                ("local s = 1 == 2 return not s", "return not(1==2)"),
                ("local s = \"a\" .. \"b\" return #s", "return#(\"a\"..\"b\")"),
                ("local s = 1 + 2 return -s ^ 2", "return-(1+2)^2"),
                ("local s = -1 return -s ^ 2", "return-(-1)^2"),
                ("local s = 1 + 2 return (s * 3)", "return((1+2)*3)"),
                ("local s = 1 + 2 print(s * 3, 4)", "print((1+2)*3,4)"),
                ("local a = 1 + 2 local b = a * 3 return b ^ 2", "return((1+2)*3)^2"),
                ("local x = 37.5 return x[\"k\"]", "return(37.5)[\"k\"]"),
                ("local x = \"s\" return x.len", "return(\"s\").len"),
                ("local x = false x:resize(9)", "(false):resize(9)"),
            ],
        );
    }

    #[test]
    fn leaves_a_value_bare_where_its_grouping_holds() {
        assert_outputs(
            inline,
            &[
                ("local s = 1 + 2 return s", "return 1+2"),
                ("local s = 1 + 2 print(s)", "print(1+2)"),
                ("local s = 1 + 2 return {s}", "return{1+2}"),
                ("local s = 1 + 2 return t[s]", "return t[1+2]"),
                ("local s = 1 * 2 return s + 3", "return 1*2+3"),
                ("local s = 1 * 2 return 3 + s", "return 3+1*2"),
                ("local s = 1 - 2 return s - 5", "return 1-2-5"),
                ("local s = 2 ^ 3 return 2 ^ s", "return 2^2^3"),
                ("local s = 1 .. 2 return 3 .. s", "return 3 ..1 ..2"),
                ("local s = 1 + 2 return s < 4", "return 1+2<4"),
                ("local s = 1 < 2 return s == true", "return 1<2==true"),
                ("local s = 2 ^ 3 return -s", "return-2^3"),
                ("local s = -2 return 3 ^ s", "return 3^-2"),
                ("local s = -2 return s * 3", "return-2*3"),
                ("local s = -1 return -s", "return- -1"),
                ("local s = (1 + 2) return s * 2", "return(1+2)*2"),
            ],
        );
    }

    #[test]
    fn inlines_into_luau_casts_and_instantiations() {
        assert_outputs(
            inline_luau,
            &[
                ("local s = 1 + 2 return s :: any", "return(1+2)::any"),
                ("local s = -1 return s :: any", "return(-1)::any"),
                ("local s = 1 :: number return s :: any", "return(1::number)::any"),
                ("local s = 1 return s :: any", "return 1::any"),
                ("local s = 1 :: number return s < 2", "return(1::number)<2"),
                ("local s = 1 :: number return 2 + s < 3", "return 2+(1::number)<3"),
                ("local s = 1 :: number return -s < 2", "return-(1::number)<2"),
                ("local s = 1 :: number return 1 < s < 3", "return 1<(1::number)<3"),
                ("local s = 2 + 1 :: number return s < 3", "return 2+(1::number)<3"),
                ("local s = -1 :: number return s < 3", "return-(1::number)<3"),
                ("local s = 2 + 1 :: number return 2 * s < 3", "return 2*(2+(1::number))<3"),
                ("local s = 1 :: number return s", "return 1::number"),
                ("local s = 1 :: number return s + 1", "return 1::number+1"),
                ("local s = 1 :: number return 1 + s", "return 1+1::number"),
                ("local s = 1 :: number return s > 2", "return 1::number>2"),
                ("local s = 1 return x :: Foo<T> < s", "return x::Foo<T><1"),
                ("local s = \"x\" return s<<T>>", "return(\"x\")<<T>>"),
                ("local s = -1 return s<<T>>", "return(-1)<<T>>"),
                ("local s = \"x\" :: T return s<<U>>", "return(\"x\"::T)<<U>>"),
                ("local s = \"x\" return s<<T>><<U>>", "return(\"x\")<<T>><<U>>"),
                ("local s = (\"x\")<<T>> return s .. \"y\"", "local s=(\"x\")<<T>>return s..\"y\""),
            ],
        );
    }

    /// `local x0 = 1`, then `local xi = x(i-1) + 1` up to `x(links)`, returned.
    fn sum_chain(links: usize) -> String {
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} + 1").unwrap();
        }
        writeln!(source, "return x{links}").unwrap();
        source
    }

    #[test]
    fn inlines_a_chain_of_as_many_locals_as_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS - 1;
        let allocator = Allocator::new();
        let source = sum_chain(links);
        let mut block = parse(&allocator, &source, LuaVersion::Lua54);
        Compressor::new(&allocator, LuaVersion::Lua54, INLINING).compress(&mut block);
        let Some(LastStatement::Return(return_stmt)) = &block.last_stmt else {
            panic!("expected a return");
        };
        let run = return_stmt.exprs.first().expect("the return has its value");
        assert_eq!(expr_nesting_depth(run), 1, "the chain splices into one run");
        assert_eq!(luck_codegen::compact(&block), format!("return 1{}", "+1".repeat(links)));

        let mut concat = String::from("local x0 = \"z\"\n");
        for (i, previous) in (1..=links).zip(0usize..) {
            writeln!(concat, "local x{i} = \"a\" .. x{previous}").unwrap();
        }
        writeln!(concat, "return x{links}").unwrap();
        assert_eq!(inline(&concat), format!("return{}\"z\"", "\"a\"..".repeat(links)));
    }

    #[test]
    fn stops_a_chain_past_the_locals_one_function_holds() {
        let links = luck_token::MAX_ACTIVE_LOCALS;
        let expected = format!("local x{links}=1{} return x{links}", "+1".repeat(links));
        assert_eq!(walk_once(&sum_chain(links), INLINING), expected);
    }

    #[test]
    fn stops_inlining_a_run_of_locals_before_it_nests_past_the_depth_cap() {
        // Each value needs parentheses where the next one uses it.
        let mut source = String::from("local x0 = 1\n");
        for (i, previous) in (1..luck_token::MAX_ACTIVE_LOCALS).zip(0usize..) {
            writeln!(source, "local x{i} = x{previous} * 2 + 1").unwrap();
        }
        writeln!(source, "return x{}", luck_token::MAX_ACTIVE_LOCALS - 1).unwrap();
        assert!(inline(&source).contains("local"), "the run must stop inlining partway");
    }
}
