use core::mem;

use luck_allocator::Box;
use luck_token::{BinOp, OperandSide, UnOp};

use crate::expr::{
    BinaryLink, BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, Suffix,
    SuffixedExpression, TableConstructor, Var,
};
use crate::function::FunctionAttribute;
use crate::one_or_more::OneOrMore;
use crate::query::{is_multi_value, is_prefix_expr};
use crate::stmt::Block;
use crate::synth::{FunctionSignature, Synth};
use crate::types::{Type, TypeArgs};

/// A table field for [`Synth::table`], which fills in the spans a real
/// [`Field`] needs.
#[derive(Debug)]
pub enum FieldSpec<'name, 'ast> {
    Positional(Expression<'ast>),
    Named(&'name str, Expression<'ast>),
    Bracketed(Expression<'ast>, Expression<'ast>),
}

/// Whether the type grammar consumes this operator's token when it directly
/// follows a printed type: `<` opens generic arguments after a named type,
/// `|` and `&` continue a union or intersection.
const fn op_token_extends_types(op: BinOp) -> bool {
    match op {
        BinOp::Lt | BinOp::BitAnd | BinOp::BitOr => true,
        BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div
        | BinOp::FloorDiv
        | BinOp::Mod
        | BinOp::Pow
        | BinOp::Concat
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::And
        | BinOp::Or
        | BinOp::BitXor
        | BinOp::Shl
        | BinOp::Shr => false,
    }
}

/// Whether `expr` prints with a type cast as its rightmost part, so the next
/// token lands directly after the cast's type.
fn ends_with_type_cast(expr: &Expression<'_>) -> bool {
    let mut current = expr;
    loop {
        current = match current {
            // Luau
            Expression::TypeCast(_) => return true,
            Expression::BinaryOp(inner) => &inner.links.last().operand,
            Expression::UnaryOp(inner) => &inner.operand,
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
            | Expression::Error(_) => return false,
        };
    }
}

impl<'ast> Synth<'ast> {
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn name_var(&self, name: &str) -> Var<'ast> {
        Var::Name(self.boxed(self.name(name)))
    }

    #[must_use]
    pub fn index_var(&self, prefix: Expression<'ast>, index: Expression<'ast>) -> Var<'ast> {
        Var::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn field_var(&self, prefix: Expression<'ast>, name: &str) -> Var<'ast> {
        Var::Suffixed(self.suffixed(prefix, Suffix::Field(self.name(name))))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn name_expr(&self, name: &str) -> Expression<'ast> {
        Expression::Name(self.boxed(self.name(name)))
    }

    #[must_use]
    pub fn index(&self, prefix: Expression<'ast>, index: Expression<'ast>) -> Expression<'ast> {
        Expression::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn field(&self, prefix: Expression<'ast>, name: &str) -> Expression<'ast> {
        Expression::Suffixed(self.suffixed(prefix, Suffix::Field(self.name(name))))
    }

    fn index_chain(
        &self,
        prefix: Expression<'ast>,
        index: Expression<'ast>,
    ) -> Box<'ast, SuffixedExpression<'ast>> {
        let span = self.next_span();
        self.suffixed(prefix, Suffix::Index { span, index })
    }

    /// `prefix` and then `suffix`, which joins `prefix`'s own run when it has
    /// one, so the run stays one node.
    fn suffixed(
        &self,
        prefix: Expression<'ast>,
        suffix: Suffix<'ast>,
    ) -> Box<'ast, SuffixedExpression<'ast>> {
        let wrapped = self.wrap_prefix(prefix);
        if let Expression::Suffixed(mut chain) = wrapped {
            chain.span = self.next_span();
            chain.suffixes.push(suffix, self.allocator);
            return chain;
        }
        let suffixes = OneOrMore::new(suffix, self.allocator);
        SuffixedExpression::boxed(self.next_span(), wrapped, suffixes, self.allocator)
    }

    /// `prefix.name` when `name` is a safe identifier (in the pinned dialect,
    /// or in every dialect when none is pinned), `prefix["name"]` otherwise.
    #[must_use]
    pub fn field_or_index(&self, prefix: Expression<'ast>, name: &str) -> Expression<'ast> {
        if self.is_safe_field_name(name) {
            self.field(prefix, name)
        } else {
            let key = self.string(name);
            self.index(prefix, key)
        }
    }

    /// `lhs op rhs`, parenthesizing either operand where precedence,
    /// associativity, a greedy if-expression, or a type cast printed before
    /// `<`, `&`, or `|` would otherwise change the parse.
    ///
    /// An operand left bare at `op`'s level joins the run, as it would read
    /// back.
    #[must_use]
    pub fn binary_op(
        &self,
        lhs: Expression<'ast>,
        op: BinOp,
        rhs: Expression<'ast>,
    ) -> Expression<'ast> {
        let operands = (
            self.wrap_binary_operand(lhs, op, OperandSide::Left),
            self.wrap_binary_operand(rhs, op, OperandSide::Right),
        );
        let span = self.next_span();
        let level = op.precedence().0;
        let run = match operands {
            (Expression::BinaryOp(mut run), operand) if run.precedence().0 == level => {
                run.span = span;
                run.links.push(BinaryLink::new(op, operand), self.allocator);
                run
            }
            (first, Expression::BinaryOp(mut run)) if run.precedence().0 == level => {
                let second = mem::replace(&mut run.first, first);
                run.span = span;
                run.links.insert(0, BinaryLink::new(op, second), self.allocator);
                run
            }
            (first, operand) => {
                let links = OneOrMore::new(BinaryLink::new(op, operand), self.allocator);
                BinaryOp::boxed(span, first, links, self.allocator)
            }
        };
        Expression::BinaryOp(run)
    }

    /// `op operand`, parenthesizing a binary or if-expression operand.
    #[must_use]
    pub fn unary_op(&self, op: UnOp, operand: Expression<'ast>) -> Expression<'ast> {
        let wrapped = match operand {
            Expression::BinaryOp(_) | Expression::IfExpression(_) => self.parenthesized(operand),
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
            | Expression::UnaryOp(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => operand,
        };
        Expression::new_unary_op(self.next_span(), op, wrapped, self.allocator)
    }

    #[must_use]
    pub fn parenthesized(&self, expr: Expression<'ast>) -> Expression<'ast> {
        Expression::new_parenthesized(self.next_span(), expr, self.allocator)
    }

    /// Truncates a call or `...`, the only multi-value forms, to one value by
    /// parenthesizing it. Anything else passes through unchanged.
    ///
    /// Use it at the end of a call, return, or assignment list when exactly
    /// one value is meant, such as a bytecode `CALL` that requests one result.
    #[must_use]
    pub fn single_value(&self, expr: Expression<'ast>) -> Expression<'ast> {
        if is_multi_value(&expr) { self.parenthesized(expr) } else { expr }
    }

    /// Luau type cast: `expr :: T`. A cast binds to a simple expression, so
    /// binary, unary, and if-expression operands get parentheses, and so do
    /// cast operands, since Luau rejects `a :: T :: U`.
    #[must_use]
    pub fn type_cast(&self, expr: Expression<'ast>, ty: Type<'ast>) -> Expression<'ast> {
        let wrapped = match expr {
            Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_) => self.parenthesized(expr),
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
            | Expression::InterpolatedString(_)
            | Expression::Error(_) => expr,
        };
        Expression::new_type_cast(self.next_span(), wrapped, ty, self.allocator)
    }

    /// Luau: `expr<<T, U>>`.
    #[must_use]
    pub fn type_instantiation<Types>(
        &self,
        expr: Expression<'ast>,
        types: Types,
    ) -> Expression<'ast>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let suffix = Suffix::TypeInstantiation { span, type_args: self.type_args(types) };
        Expression::Suffixed(self.suffixed(expr, suffix))
    }

    pub(super) fn type_args<Types>(&self, types: Types) -> Box<'ast, TypeArgs<'ast>>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
    {
        TypeArgs::boxed(self.next_span(), self.list(types), self.allocator)
    }

    fn wrap_binary_operand(
        &self,
        operand: Expression<'ast>,
        parent_op: BinOp,
        side: OperandSide,
    ) -> Expression<'ast> {
        let should_parenthesize = match &operand {
            Expression::BinaryOp(inner) => !inner.can_nest_bare(parent_op, side),
            // Only `^` binds tighter than unary; `a ^ -b` needs no parens
            // because a unary operator is always accepted after a binop.
            Expression::UnaryOp(_) => {
                side == OperandSide::Left && parent_op.precedence().0 > UnOp::PRECEDENCE
            }
            // Luau: an if-expression extends as far right as it can, so
            // anything printed after it lands in its else branch. The right
            // side gets wrapped too, since a later composition may print
            // after it.
            Expression::IfExpression(_) => true,
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
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => false,
        } || (side == OperandSide::Left
            && op_token_extends_types(parent_op)
            && ends_with_type_cast(&operand));
        if should_parenthesize { self.parenthesized(operand) } else { operand }
    }

    fn wrap_prefix(&self, prefix: Expression<'ast>) -> Expression<'ast> {
        if is_prefix_expr(&prefix) { prefix } else { self.parenthesized(prefix) }
    }

    #[must_use]
    pub fn call<Args>(&self, callee: Expression<'ast>, args: Args) -> Expression<'ast>
    where
        Args: IntoIterator<Item = Expression<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        Expression::Suffixed(self.suffixed(callee, Suffix::Call(self.parenthesized_args(args))))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn method_call<Args>(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        args: Args,
    ) -> Expression<'ast>
    where
        Args: IntoIterator<Item = Expression<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        self.method_call_node(receiver, name, None, self.parenthesized_args(args))
    }

    /// Luau: `receiver:name<<T, U>>(args)`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn method_call_with_types<Types, Args>(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        types: Types,
        args: Args,
    ) -> Expression<'ast>
    where
        Types: IntoIterator<Item = Type<'ast>>,
        Types::IntoIter: ExactSizeIterator,
        Args: IntoIterator<Item = Expression<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        let type_args = self.type_args(types);
        self.method_call_node(receiver, name, Some(type_args), self.parenthesized_args(args))
    }

    /// # Panics
    ///
    /// Panics if a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn call_table<'name, Fields>(
        &self,
        callee: Expression<'ast>,
        fields: Fields,
    ) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = FieldSpec<'name, 'ast>>,
        Fields::IntoIter: ExactSizeIterator,
    {
        let args = FunctionArgs::TableConstructor(self.table_constructor(fields));
        Expression::Suffixed(self.suffixed(callee, Suffix::Call(args)))
    }

    /// `callee"content"`, the string argument form (content is unquoted).
    #[must_use]
    pub fn call_string(&self, callee: Expression<'ast>, content: &str) -> Expression<'ast> {
        let args = FunctionArgs::StringLiteral(self.quoted(content.as_bytes()));
        Expression::Suffixed(self.suffixed(callee, Suffix::Call(args)))
    }

    /// # Panics
    ///
    /// Panics if `name` or a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn method_call_table<'name, Fields>(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        fields: Fields,
    ) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = FieldSpec<'name, 'ast>>,
        Fields::IntoIter: ExactSizeIterator,
    {
        let args = FunctionArgs::TableConstructor(self.table_constructor(fields));
        self.method_call_node(receiver, name, None, args)
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn method_call_string(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        content: &str,
    ) -> Expression<'ast> {
        let args = FunctionArgs::StringLiteral(self.quoted(content.as_bytes()));
        self.method_call_node(receiver, name, None, args)
    }

    fn method_call_node(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        type_args: Option<Box<'ast, TypeArgs<'ast>>>,
        args: FunctionArgs<'ast>,
    ) -> Expression<'ast> {
        let call = Suffix::new_method_call(self.name(name), type_args, args, self.allocator);
        Expression::Suffixed(self.suffixed(receiver, call))
    }

    fn parenthesized_args<Args>(&self, args: Args) -> FunctionArgs<'ast>
    where
        Args: IntoIterator<Item = Expression<'ast>>,
        Args::IntoIter: ExactSizeIterator,
    {
        FunctionArgs::Parenthesized { span: self.next_span(), args: self.list(args) }
    }

    /// # Panics
    ///
    /// Panics if a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn table<'name, Fields>(&self, fields: Fields) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = FieldSpec<'name, 'ast>>,
        Fields::IntoIter: ExactSizeIterator,
    {
        Expression::TableConstructor(self.table_constructor(fields))
    }

    #[must_use]
    pub fn array<Values>(&self, values: Values) -> Expression<'ast>
    where
        Values: IntoIterator<Item = Expression<'ast>>,
        Values::IntoIter: ExactSizeIterator,
    {
        self.table(values.into_iter().map(FieldSpec::Positional))
    }

    /// Keys that aren't safe identifiers fall back to the bracketed string form.
    #[must_use]
    pub fn record<'name, Fields>(&self, fields: Fields) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = (&'name str, Expression<'ast>)>,
        Fields::IntoIter: ExactSizeIterator,
    {
        self.table(fields.into_iter().map(|(name, value)| {
            if self.is_safe_field_name(name) {
                FieldSpec::Named(name, value)
            } else {
                FieldSpec::Bracketed(self.string(name), value)
            }
        }))
    }

    fn table_constructor<'name, Specs>(&self, specs: Specs) -> Box<'ast, TableConstructor<'ast>>
    where
        Specs: IntoIterator<Item = FieldSpec<'name, 'ast>>,
        Specs::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let fields = self.list(specs.into_iter().map(|spec| match spec {
            FieldSpec::Positional(value) => Field::Positional(value),
            FieldSpec::Named(name, value) => Field::Named { name: self.name(name), value },
            FieldSpec::Bracketed(key, value) => {
                Field::Bracketed { span: self.next_span(), key, value }
            }
        }));
        TableConstructor::boxed(span, fields, false, self.allocator)
    }

    /// For attributes, generics, typed parameters, varargs, or a return type, use
    /// [`Synth::function_def_full`].
    ///
    /// # Panics
    ///
    /// Panics if a name in `params` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn function_def(&self, params: &[&str], body: Block<'ast>) -> Expression<'ast> {
        let signature = FunctionSignature { params: self.params(params), ..Default::default() };
        self.function_def_full([], signature, body)
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    #[must_use]
    pub fn function_def_full<Attributes>(
        &self,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Expression<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        Expression::new_function_def(
            self.next_span(),
            self.list(attributes),
            self.function_body(signature, body),
            self.allocator,
        )
    }

    /// Luau: `if condition then then_expr elseif ... else else_expr`.
    #[must_use]
    pub fn if_expr<ElseIfs>(
        &self,
        condition: Expression<'ast>,
        then_expr: Expression<'ast>,
        elseifs: ElseIfs,
        else_expr: Expression<'ast>,
    ) -> Expression<'ast>
    where
        ElseIfs: IntoIterator<Item = (Expression<'ast>, Expression<'ast>)>,
        ElseIfs::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let elseif_clauses = self.list(elseifs.into_iter().map(|(elseif_condition, expr)| {
            ElseIfExprClause::new(self.next_span(), elseif_condition, expr)
        }));
        Expression::new_if_expr(
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
            self.allocator,
        )
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;
    use crate::fixtures::Fixture;

    fn chain<'tree, 'ast>(expr: &'tree Expression<'ast>) -> &'tree SuffixedExpression<'ast> {
        let Expression::Suffixed(chain) = expr else {
            panic!("expected a suffixed expression, got {expr:?}");
        };
        chain
    }

    fn run<'tree, 'ast>(expr: &'tree Expression<'ast>) -> &'tree BinaryOp<'ast> {
        let Expression::BinaryOp(run) = expr else {
            panic!("expected a binary run, got {expr:?}");
        };
        run
    }

    #[test]
    fn binary_op_parenthesizes_lower_precedence_operands() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // (a + b) * c: the additive LHS must gain parens under `*`.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let product = synth.binary_op(sum, BinOp::Mul, synth.name_expr("c"));
        assert!(
            matches!(run(&product).first, Expression::Parenthesized(_)),
            "lower-precedence LHS not parenthesized"
        );
    }

    #[test]
    fn binary_op_keeps_higher_precedence_operands_bare() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // a * b + c: the multiplicative LHS needs no parens under `+`.
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, synth.name_expr("b"));
        let sum = synth.binary_op(product, BinOp::Add, synth.name_expr("c"));
        assert!(matches!(run(&sum).first, Expression::BinaryOp(_)));
    }

    #[test]
    fn binary_op_honors_associativity() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // Left-nested concat needs parens: (a .. b) .. c is not a .. b .. c.
        let inner_concat =
            synth.binary_op(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b"));
        let concat = synth.binary_op(inner_concat, BinOp::Concat, synth.name_expr("c"));
        assert!(matches!(run(&concat).first, Expression::Parenthesized(_)));

        // Right-nested subtraction needs parens: a - (b - c).
        let inner_difference =
            synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        let difference = synth.binary_op(synth.name_expr("a"), BinOp::Sub, inner_difference);
        assert!(matches!(run(&difference).links.last().operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn binary_op_parenthesizes_unary_lhs_only_under_caret() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // (-a)^b: without parens this would re-parse as -(a^b).
        let negated_base = synth.unary_op(UnOp::Neg, synth.name_expr("a"));
        let power = synth.binary_op(negated_base, BinOp::Pow, synth.name_expr("b"));
        assert!(matches!(run(&power).first, Expression::Parenthesized(_)));

        // a * -b stays bare: unary is always accepted after a binary_op.
        let negated_factor = synth.unary_op(UnOp::Neg, synth.name_expr("b"));
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, negated_factor);
        assert!(matches!(run(&product).links.last().operand, Expression::UnaryOp(_)));
    }

    #[test]
    fn binary_op_parenthesizes_if_expr_operands() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let if_expr = synth.if_expr(synth.boolean(true), synth.number("1"), [], synth.number("2"));
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, if_expr);
        assert!(
            matches!(run(&sum).links.last().operand, Expression::Parenthesized(_)),
            "greedy if-expression must be parenthesized as an operand"
        );
    }

    #[test]
    fn binary_op_parenthesizes_casts_before_type_extending_ops() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // (a :: T) < b: bare, the type parser reads `<` as generic args.
        for op in [BinOp::Lt, BinOp::BitAnd, BinOp::BitOr] {
            let cast = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
            let compared = synth.binary_op(cast, op, synth.name_expr("b"));
            assert!(
                matches!(run(&compared).first, Expression::Parenthesized(_)),
                "cast before {op:?} must be parenthesized"
            );
        }

        // a + b :: T stays bare: `+` can't extend a printed type.
        let right_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let bare_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, right_cast);
        assert!(matches!(run(&bare_sum).links.last().operand, Expression::TypeCast(_)));

        // The cast may sit at the end of a larger left operand: the whole
        // operand gains parens, `(a + b :: T) < c`.
        let trailing_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, trailing_cast);
        let sum_compared = synth.binary_op(cast_sum, BinOp::Lt, synth.name_expr("c"));
        assert!(matches!(run(&sum_compared).first, Expression::Parenthesized(_)));

        // A cast on the right of `<` ends the expression; no parens needed.
        let rhs_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_compared = synth.binary_op(synth.name_expr("a"), BinOp::Lt, rhs_cast);
        assert!(matches!(run(&cast_compared).links.last().operand, Expression::TypeCast(_)));
    }

    #[test]
    fn unary_op_parenthesizes_binary_operand() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // -(a + b): without parens this would re-parse as (-a) + b.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let Expression::UnaryOp(unary_op) = synth.unary_op(UnOp::Neg, sum) else {
            panic!("expected unop");
        };
        assert!(matches!(unary_op.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_compound_operand() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let Expression::TypeCast(type_cast) = synth.type_cast(sum, synth.ty_named("number")) else {
            panic!("expected type cast");
        };
        assert!(matches!(type_cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_cast_operands() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // (a :: T) :: U: `a :: T :: U` is a parse error in Luau.
        let inner = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
        let Expression::TypeCast(cast) = synth.type_cast(inner, synth.ty_named("U")) else {
            panic!("expected type cast");
        };
        assert!(matches!(cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn single_value_wraps_multi_value_forms() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let call = synth.call(synth.name_expr("f"), []);
        assert!(matches!(synth.single_value(call), Expression::Parenthesized(_)));
        assert!(matches!(synth.single_value(synth.vararg()), Expression::Parenthesized(_)));
        // Single-value expressions pass through unchanged.
        assert!(matches!(synth.single_value(synth.name_expr("x")), Expression::Name(_)));
        assert!(matches!(synth.single_value(synth.number_i64(1)), Expression::Number(_)));
    }

    #[test]
    fn prefix_positions_wrap_non_prefix_exprs() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        // A string receiver must be parenthesized: ("s"):rep(2).
        let called = synth.method_call(synth.string("s"), "rep", [synth.number("2")]);
        assert!(matches!(chain(&called).prefix, Expression::Parenthesized(_)));

        // A table prefix must be parenthesized: ({}).x.
        let accessed = synth.field(synth.table([]), "x");
        assert!(matches!(chain(&accessed).prefix, Expression::Parenthesized(_)));
    }

    #[test]
    fn a_suffix_joins_the_run_it_follows() {
        // f().x[1]:m(), one run after `f`.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let called = synth.call(synth.name_expr("f"), []);
        let indexed = synth.index(synth.field(called, "x"), synth.number("1"));
        let method_run = synth.method_call(indexed, "m", []);
        assert!(matches!(chain(&method_run).prefix, Expression::Name(_)));
        assert!(matches!(
            *chain(&method_run).suffixes,
            [Suffix::Call(_), Suffix::Field(_), Suffix::Index { .. }, Suffix::MethodCall(_)]
        ));
    }

    #[test]
    fn an_operand_bare_at_the_same_level_joins_the_run() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let run_len = |expr: &Expression<'_>| run(expr).links.len();
        // (a + b) - c reads back as one run.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        assert_eq!(run_len(&synth.binary_op(sum, BinOp::Sub, synth.name_expr("c"))), 2);
        // So does a .. (b .. c).
        let concat = synth.binary_op(synth.name_expr("b"), BinOp::Concat, synth.name_expr("c"));
        assert_eq!(run_len(&synth.binary_op(synth.name_expr("a"), BinOp::Concat, concat)), 2);
        // a - (b - c) keeps its parentheses, and with them a run of its own.
        let difference = synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        assert_eq!(run_len(&synth.binary_op(synth.name_expr("a"), BinOp::Sub, difference)), 1);
    }

    #[test]
    fn a_run_extended_a_link_at_a_time_keeps_its_order() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let names: Vec<String> = (0..2_000u32).map(|i| format!("v{i}")).collect();
        let name_of = |expr: &Expression<'_>| {
            let Expression::Name(name) = expr else {
                panic!("expected a name, got {expr:?}");
            };
            name.text.as_str().to_owned()
        };
        let operand_names = |expr: &Expression<'_>| {
            let run = run(expr);
            let mut operands = vec![name_of(&run.first)];
            operands.extend(run.links.iter().map(|link| name_of(&link.operand)));
            operands
        };
        let (first, rest) = names.split_first().expect("names");
        let (last, before_last) = names.split_last().expect("names");

        let mut sum = synth.name_expr(first);
        for name in rest {
            sum = synth.binary_op(sum, BinOp::Add, synth.name_expr(name));
        }
        assert_eq!(operand_names(&sum), names);

        let mut concat = synth.name_expr(last);
        for name in before_last.iter().rev() {
            concat = synth.binary_op(synth.name_expr(name), BinOp::Concat, concat);
        }
        assert_eq!(operand_names(&concat), names);

        let mut path = synth.name_expr("t");
        for name in &names {
            path = synth.field(path, name);
        }
        let fields: Vec<&str> = chain(&path)
            .suffixes
            .iter()
            .map(|suffix| {
                let Suffix::Field(name) = suffix else {
                    panic!("expected a field, got {suffix:?}");
                };
                name.text.as_str()
            })
            .collect();
        assert_eq!(fields, names);
    }

    #[test]
    fn a_run_extended_at_its_end_takes_linear_arena_space() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let mut sum = synth.name_expr("x");
        let one_link = fixture.allocator().used_bytes();
        for _ in 0..10_000u32 {
            sum = synth.binary_op(sum, BinOp::Add, synth.nil());
        }
        assert_eq!(run(&sum).links.len(), 10_000);
        // The links' doubling allocations and the run's box, well under the
        // quadratic space that copying the run at each link would take.
        let used = fixture.allocator().used_bytes().saturating_sub(one_link);
        assert!(used < 10_000 * 4 * size_of::<BinaryLink<'_>>(), "{used} bytes");
    }

    #[test]
    fn field_or_index_picks_form_by_name() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let valid = synth.field_or_index(synth.name_expr("t"), "valid_name");
        assert!(matches!(chain(&valid).suffixes.last(), Suffix::Field(_)));
        for bad in ["not an ident", "1st", "", "end", "goto"] {
            let bracketed = synth.field_or_index(synth.name_expr("t"), bad);
            assert!(
                matches!(chain(&bracketed).suffixes.last(), Suffix::Index { .. }),
                "{bad:?} should use the bracketed form"
            );
        }
    }

    #[test]
    fn field_or_index_follows_the_pinned_version() {
        let fixture = Fixture::default();
        // Unpinned: `goto` is a keyword somewhere, so it gets bracketed.
        let unpinned = fixture.synth();
        let unpinned_goto = unpinned.field_or_index(unpinned.name_expr("t"), "goto");
        assert!(matches!(chain(&unpinned_goto).suffixes.last(), Suffix::Index { .. }));

        // Pinned to Luau: `goto` is a plain identifier there.
        let luau = fixture.synth().with_version(LuaVersion::Luau);
        let luau_goto = luau.field_or_index(luau.name_expr("t"), "goto");
        assert!(matches!(chain(&luau_goto).suffixes.last(), Suffix::Field(_)));

        // Pinned to 5.4: still a keyword, still bracketed.
        let lua54 = fixture.synth().with_version(LuaVersion::Lua54);
        let lua54_goto = lua54.field_or_index(lua54.name_expr("t"), "goto");
        assert!(matches!(chain(&lua54_goto).suffixes.last(), Suffix::Index { .. }));
    }

    #[test]
    fn builds_sugar_call_forms() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let string_call = synth.call_string(synth.name_expr("require"), "module");
        assert!(matches!(
            chain(&string_call).suffixes.last(),
            Suffix::Call(FunctionArgs::StringLiteral(_))
        ));

        let table_call = synth.call_table(
            synth.name_expr("configure"),
            [FieldSpec::Named("debug", synth.boolean(true))],
        );
        assert!(matches!(
            chain(&table_call).suffixes.last(),
            Suffix::Call(FunctionArgs::TableConstructor(_))
        ));

        let method_call = synth.method_call_string(synth.name_expr("s"), "gsub", "pattern");
        let Suffix::MethodCall(call) = chain(&method_call).suffixes.last() else {
            panic!("expected a method call");
        };
        assert_eq!(call.name.text, "gsub");
        assert!(matches!(call.args, FunctionArgs::StringLiteral(_)));
    }

    #[test]
    fn builds_mixed_table() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let table = synth.table([
            FieldSpec::Positional(synth.number("1")),
            FieldSpec::Named("a", synth.number("2")),
            FieldSpec::Bracketed(synth.string("k"), synth.boolean(true)),
        ]);
        let Expression::TableConstructor(constructor) = &table else {
            panic!("expected table constructor");
        };
        assert!(matches!(
            *constructor.fields,
            [Field::Positional(_), Field::Named { .. }, Field::Bracketed { .. }]
        ));
    }

    #[test]
    fn array_is_positional_and_record_brackets_unsafe_keys() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let Expression::TableConstructor(array) =
            synth.array([synth.number("1"), synth.number("2")])
        else {
            panic!("expected table");
        };
        assert!(array.fields.iter().all(|field| matches!(field, Field::Positional(_))));

        // Unsafe keys fall back to the bracketed form.
        let Expression::TableConstructor(record) =
            synth.record([("good", synth.number("1")), ("not valid", synth.number("2"))])
        else {
            panic!("expected table");
        };
        assert!(matches!(*record.fields, [Field::Named { .. }, Field::Bracketed { .. }]));
    }

    #[test]
    fn builds_if_expr_with_elseif() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let expr = synth.if_expr(
            synth.boolean(true),
            synth.number("1"),
            [(synth.boolean(false), synth.number("2"))],
            synth.number("3"),
        );
        let Expression::IfExpression(if_expr) = &expr else {
            panic!("expected if expression");
        };
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    }

    #[test]
    fn builds_type_instantiations_and_typed_method_calls() {
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        let instantiated =
            synth.type_instantiation(synth.name_expr("f"), [synth.ty_named("integer")]);
        let Suffix::TypeInstantiation { type_args, .. } = chain(&instantiated).suffixes.last()
        else {
            panic!("expected type instantiation");
        };
        assert_eq!(type_args.args.len(), 1);

        let method = synth.method_call_with_types(
            synth.name_expr("receiver"),
            "map",
            [synth.ty_named("string")],
            [synth.string("value")],
        );
        let Suffix::MethodCall(call) = chain(&method).suffixes.last() else {
            panic!("expected a method call");
        };
        assert_eq!(call.type_args.as_ref().map(|method_types| method_types.args.len()), Some(1));
    }
}
