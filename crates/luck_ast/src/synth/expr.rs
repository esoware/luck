use luck_token::{BinOp, OperandSide, UnOp, encode_string};

use crate::expr::{
    BinaryLink, BinaryOp, CallSuffix, ElseIfExprClause, Expression, Field, FunctionArgs,
    FunctionDef, IfExpression, Literal, ParenthesizedExpression, Suffix, SuffixedExpression,
    TableConstructor, TypeCast, UnaryOp, Var,
};
use crate::function::FunctionAttribute;
use crate::one_or_more::OneOrMore;
use crate::punctuated::Punctuated;
use crate::query::{is_multi_value, is_prefix_expr};
use crate::stmt::Block;
use crate::synth::{FunctionSignature, Synth};
use crate::types::{Type, TypeArgs};

/// A table field for [`Synth::table`], which fills in the spans and
/// separators a real [`Field`] needs.
#[derive(Debug)]
pub enum FieldSpec<'name> {
    Positional(Expression),
    Named(&'name str, Expression),
    Bracketed(Expression, Expression),
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
fn ends_with_type_cast(expr: &Expression) -> bool {
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

impl Synth {
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn name_var(&self, name: &str) -> Var {
        Var::Name(self.identifier(name))
    }

    #[must_use]
    pub fn index_var(&self, prefix: Expression, index: Expression) -> Var {
        Var::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn field_var(&self, prefix: Expression, name: &str) -> Var {
        Var::Suffixed(self.suffixed(prefix, Suffix::Field(self.identifier(name))))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn name_expr(&self, name: &str) -> Expression {
        Expression::Name(self.identifier(name))
    }

    #[must_use]
    pub fn index(&self, prefix: Expression, index: Expression) -> Expression {
        Expression::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn field(&self, prefix: Expression, name: &str) -> Expression {
        Expression::Suffixed(self.suffixed(prefix, Suffix::Field(self.identifier(name))))
    }

    fn index_chain(&self, prefix: Expression, index: Expression) -> Box<SuffixedExpression> {
        let span = self.next_span();
        self.suffixed(prefix, Suffix::Index { span, index })
    }

    /// `prefix` and then `suffix`, which joins `prefix`'s own run when it has
    /// one, so the run stays one node. Joining copies the suffixes before it.
    fn suffixed(&self, prefix: Expression, suffix: Suffix) -> Box<SuffixedExpression> {
        let wrapped = self.wrap_prefix(prefix);
        if let Expression::Suffixed(mut chain) = wrapped {
            chain.span = self.next_span();
            chain.suffixes.push(suffix);
            return chain;
        }
        Box::new(SuffixedExpression {
            span: self.next_span(),
            prefix: wrapped,
            suffixes: OneOrMore::new(suffix),
        })
    }

    /// `prefix.name` when `name` is a safe identifier (in the pinned dialect,
    /// or in every dialect when none is pinned), `prefix["name"]` otherwise.
    #[must_use]
    pub fn field_or_index(&self, prefix: Expression, name: &str) -> Expression {
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
    /// back. Joining copies the links of the operand's run.
    #[must_use]
    pub fn binary_op(&self, lhs: Expression, op: BinOp, rhs: Expression) -> Expression {
        let operands = (
            self.wrap_binary_operand(lhs, op, OperandSide::Left),
            self.wrap_binary_operand(rhs, op, OperandSide::Right),
        );
        let span = self.next_span();
        let level = op.precedence().0;
        let run = match operands {
            (Expression::BinaryOp(mut run), operand) if run.precedence().0 == level => {
                run.span = span;
                run.links.push(BinaryLink { op, operand });
                run
            }
            (first, Expression::BinaryOp(run)) if run.precedence().0 == level => {
                let BinaryOp { first: second, links, .. } = *run;
                let joined_links =
                    OneOrMore::from_first(BinaryLink { op, operand: second }, links.into_vec());
                Box::new(BinaryOp { span, first, links: joined_links })
            }
            (first, operand) => Box::new(BinaryOp {
                span,
                first,
                links: OneOrMore::new(BinaryLink { op, operand }),
            }),
        };
        Expression::BinaryOp(run)
    }

    /// `op operand`, parenthesizing a binary or if-expression operand.
    #[must_use]
    pub fn unary_op(&self, op: UnOp, operand: Expression) -> Expression {
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
        Expression::UnaryOp(Box::new(UnaryOp { span: self.next_span(), op, operand: wrapped }))
    }

    #[must_use]
    pub fn parenthesized(&self, expr: Expression) -> Expression {
        Expression::Parenthesized(Box::new(ParenthesizedExpression {
            span: self.next_span(),
            expr,
        }))
    }

    /// Truncates a call or `...`, the only multi-value forms, to one value by
    /// parenthesizing it. Anything else passes through unchanged.
    ///
    /// Use it at the end of a call, return, or assignment list when exactly
    /// one value is meant, such as a bytecode `CALL` that requests one result.
    #[must_use]
    pub fn single_value(&self, expr: Expression) -> Expression {
        if is_multi_value(&expr) { self.parenthesized(expr) } else { expr }
    }

    /// Luau type cast: `expr :: T`. A cast binds to a simple expression, so
    /// binary, unary, and if-expression operands get parentheses, and so do
    /// cast operands, since Luau rejects `a :: T :: U`.
    #[must_use]
    pub fn type_cast(&self, expr: Expression, type_value: Type) -> Expression {
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
        Expression::TypeCast(Box::new(TypeCast {
            span: self.next_span(),
            expr: wrapped,
            type_annotation: type_value,
        }))
    }

    #[must_use]
    pub fn type_instantiation(&self, expr: Expression, type_args: Vec<Type>) -> Expression {
        let suffix = Suffix::TypeInstantiation {
            span: self.next_span(),
            type_args: TypeArgs { span: self.next_span(), args: Punctuated::from_items(type_args) },
        };
        Expression::Suffixed(self.suffixed(expr, suffix))
    }

    fn wrap_binary_operand(
        &self,
        operand: Expression,
        parent_op: BinOp,
        side: OperandSide,
    ) -> Expression {
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

    fn wrap_prefix(&self, prefix: Expression) -> Expression {
        if is_prefix_expr(&prefix) { prefix } else { self.parenthesized(prefix) }
    }

    #[must_use]
    pub fn call(&self, callee: Expression, args: Vec<Expression>) -> Expression {
        self.call_node(callee, None, None, self.parenthesized_args(args))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn method_call(
        &self,
        receiver: Expression,
        name: &str,
        args: Vec<Expression>,
    ) -> Expression {
        self.call_node(receiver, Some(name), None, self.parenthesized_args(args))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn method_call_with_types(
        &self,
        receiver: Expression,
        name: &str,
        type_args: Vec<Type>,
        args: Vec<Expression>,
    ) -> Expression {
        self.call_node(receiver, Some(name), Some(type_args), self.parenthesized_args(args))
    }

    /// # Panics
    ///
    /// Panics if a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn call_table(&self, callee: Expression, fields: Vec<FieldSpec<'_>>) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_constructor(fields)));
        self.call_node(callee, None, None, args)
    }

    /// `callee"content"`, the string argument form (content is unquoted).
    #[must_use]
    pub fn call_string(&self, callee: Expression, content: &str) -> Expression {
        let args = FunctionArgs::StringLiteral(Literal {
            text: encode_string(content.as_bytes()),
            span: self.next_span(),
        });
        self.call_node(callee, None, None, args)
    }

    /// # Panics
    ///
    /// Panics if `name` or a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn method_call_table(
        &self,
        receiver: Expression,
        name: &str,
        fields: Vec<FieldSpec<'_>>,
    ) -> Expression {
        let args = FunctionArgs::TableConstructor(Box::new(self.table_constructor(fields)));
        self.call_node(receiver, Some(name), None, args)
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn method_call_string(
        &self,
        receiver: Expression,
        name: &str,
        content: &str,
    ) -> Expression {
        let args = FunctionArgs::StringLiteral(Literal {
            text: encode_string(content.as_bytes()),
            span: self.next_span(),
        });
        self.call_node(receiver, Some(name), None, args)
    }

    fn call_node(
        &self,
        callee: Expression,
        method: Option<&str>,
        type_args: Option<Vec<Type>>,
        args: FunctionArgs,
    ) -> Expression {
        let call = CallSuffix {
            method: method.map(|name| self.identifier(name)),
            explicit_type_args: type_args.map(|types| {
                Box::new(TypeArgs { span: self.next_span(), args: Punctuated::from_items(types) })
            }),
            args,
        };
        Expression::Suffixed(self.suffixed(callee, Suffix::Call(call)))
    }

    fn parenthesized_args(&self, args: Vec<Expression>) -> FunctionArgs {
        FunctionArgs::Parenthesized { span: self.next_span(), args: Punctuated::from_items(args) }
    }

    /// # Panics
    ///
    /// Panics if a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn table(&self, fields: Vec<FieldSpec<'_>>) -> Expression {
        Expression::TableConstructor(Box::new(self.table_constructor(fields)))
    }

    #[must_use]
    pub fn array(&self, values: Vec<Expression>) -> Expression {
        self.table(values.into_iter().map(FieldSpec::Positional).collect())
    }

    /// Keys that aren't safe identifiers fall back to the bracketed string form.
    #[must_use]
    pub fn record(&self, fields: Vec<(&str, Expression)>) -> Expression {
        self.table(
            fields
                .into_iter()
                .map(|(name, v)| {
                    if self.is_safe_field_name(name) {
                        FieldSpec::Named(name, v)
                    } else {
                        FieldSpec::Bracketed(self.string(name), v)
                    }
                })
                .collect(),
        )
    }

    fn table_constructor(&self, fields: Vec<FieldSpec<'_>>) -> TableConstructor {
        let span = self.next_span();
        let built_fields = fields
            .into_iter()
            .map(|field| match field {
                FieldSpec::Positional(value) => Field::Positional { span: self.next_span(), value },
                FieldSpec::Named(name, value) => {
                    Field::Named { span: self.next_span(), name: self.identifier(name), value }
                }
                FieldSpec::Bracketed(key, value) => {
                    Field::Bracketed { span: self.next_span(), key, value }
                }
            })
            .collect();
        TableConstructor { span, fields: Punctuated::from_items(built_fields) }
    }

    /// For attributes, generics, typed parameters, varargs, or a return type, use
    /// [`Synth::function_def_full`].
    ///
    /// # Panics
    ///
    /// Panics if a name in `params` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn function_def(&self, params: &[&str], body: Block) -> Expression {
        self.function_def_full(
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    #[must_use]
    pub fn function_def_full(
        &self,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Expression {
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            attributes,
            body: self.function_body(signature, body),
        }))
    }

    #[must_use]
    pub fn if_expr(
        &self,
        condition: Expression,
        then_expr: Expression,
        elseifs: Vec<(Expression, Expression)>,
        else_expr: Expression,
    ) -> Expression {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfExprClause> = elseifs
            .into_iter()
            .map(|(elseif_condition, expr)| ElseIfExprClause {
                span: self.next_span(),
                condition: elseif_condition,
                expr,
            })
            .collect();
        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
        }))
    }
}

#[cfg(test)]
mod tests {
    use luck_token::LuaVersion;

    use super::*;

    fn chain(expr: &Expression) -> &SuffixedExpression {
        let Expression::Suffixed(chain) = expr else {
            panic!("expected a suffixed expression, got {expr:?}");
        };
        chain
    }

    fn last_call(expr: &Expression) -> &CallSuffix {
        let Suffix::Call(call) = chain(expr).suffixes.last() else {
            panic!("expected a call, got {expr:?}");
        };
        call
    }

    #[test]
    fn binary_op_parenthesizes_lower_precedence_operands() {
        let synth = Synth::new();
        // (a + b) * c: the additive LHS must gain parens under `*`.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let product = synth.binary_op(sum, BinOp::Mul, synth.name_expr("c"));

        let Expression::BinaryOp(binary_op) = &product else {
            panic!("expected binop");
        };
        assert!(
            matches!(binary_op.first, Expression::Parenthesized(_)),
            "lower-precedence LHS not parenthesized"
        );
    }

    #[test]
    fn binary_op_keeps_higher_precedence_operands_bare() {
        let synth = Synth::new();
        // a * b + c: the multiplicative LHS needs no parens under `+`.
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, synth.name_expr("b"));
        let sum = synth.binary_op(product, BinOp::Add, synth.name_expr("c"));

        let Expression::BinaryOp(binary_op) = &sum else {
            panic!("expected binop");
        };
        assert!(matches!(binary_op.first, Expression::BinaryOp(_)));
    }

    #[test]
    fn binary_op_honors_associativity() {
        let synth = Synth::new();
        // Left-nested concat needs parens: (a .. b) .. c is not a .. b .. c.
        let inner_concat =
            synth.binary_op(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b"));
        let concat = synth.binary_op(inner_concat, BinOp::Concat, synth.name_expr("c"));
        let Expression::BinaryOp(concat_node) = &concat else {
            panic!("expected binop");
        };
        assert!(matches!(concat_node.first, Expression::Parenthesized(_)));

        // Right-nested subtraction needs parens: a - (b - c).
        let inner_difference =
            synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        let difference = synth.binary_op(synth.name_expr("a"), BinOp::Sub, inner_difference);
        let Expression::BinaryOp(difference_node) = &difference else {
            panic!("expected binop");
        };
        assert!(matches!(difference_node.links.last().operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn binary_op_parenthesizes_unary_lhs_only_under_caret() {
        let synth = Synth::new();
        // (-a)^b: without parens this would re-parse as -(a^b).
        let negated_base = synth.unary_op(UnOp::Neg, synth.name_expr("a"));
        let power = synth.binary_op(negated_base, BinOp::Pow, synth.name_expr("b"));
        let Expression::BinaryOp(power_node) = &power else {
            panic!("expected binop");
        };
        assert!(matches!(power_node.first, Expression::Parenthesized(_)));

        // a * -b stays bare: unary is always accepted after a binary_op.
        let negated_factor = synth.unary_op(UnOp::Neg, synth.name_expr("b"));
        let product = synth.binary_op(synth.name_expr("a"), BinOp::Mul, negated_factor);
        let Expression::BinaryOp(product_node) = &product else {
            panic!("expected binop");
        };
        assert!(matches!(product_node.links.last().operand, Expression::UnaryOp(_)));
    }

    #[test]
    fn binary_op_parenthesizes_if_expr_operands() {
        let synth = Synth::new();
        let if_expr =
            synth.if_expr(synth.boolean(true), synth.number("1"), vec![], synth.number("2"));
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, if_expr);
        let Expression::BinaryOp(binary_op) = &sum else {
            panic!("expected binop");
        };
        assert!(
            matches!(binary_op.links.last().operand, Expression::Parenthesized(_)),
            "greedy if-expression must be parenthesized as an operand"
        );
    }

    #[test]
    fn binary_op_parenthesizes_casts_before_type_extending_ops() {
        let synth = Synth::new();
        // (a :: T) < b: bare, the type parser reads `<` as generic args.
        for op in [BinOp::Lt, BinOp::BitAnd, BinOp::BitOr] {
            let cast = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
            let compared = synth.binary_op(cast, op, synth.name_expr("b"));
            let Expression::BinaryOp(binary_op) = &compared else {
                panic!("expected binop");
            };
            assert!(
                matches!(binary_op.first, Expression::Parenthesized(_)),
                "cast before {op:?} must be parenthesized"
            );
        }

        // a + b :: T stays bare: `+` can't extend a printed type.
        let right_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let bare_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, right_cast);
        let Expression::BinaryOp(bare_node) = &bare_sum else {
            panic!("expected binop");
        };
        assert!(matches!(bare_node.links.last().operand, Expression::TypeCast(_)));

        // The cast may sit at the end of a larger left operand: the whole
        // operand gains parens, `(a + b :: T) < c`.
        let trailing_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, trailing_cast);
        let sum_compared = synth.binary_op(cast_sum, BinOp::Lt, synth.name_expr("c"));
        let Expression::BinaryOp(sum_node) = &sum_compared else {
            panic!("expected binop");
        };
        assert!(matches!(sum_node.first, Expression::Parenthesized(_)));

        // A cast on the right of `<` ends the expression; no parens needed.
        let rhs_cast = synth.type_cast(synth.name_expr("b"), synth.ty_named("T"));
        let cast_compared = synth.binary_op(synth.name_expr("a"), BinOp::Lt, rhs_cast);
        let Expression::BinaryOp(compared_node) = &cast_compared else {
            panic!("expected binop");
        };
        assert!(matches!(compared_node.links.last().operand, Expression::TypeCast(_)));
    }

    #[test]
    fn unary_op_parenthesizes_binary_operand() {
        let synth = Synth::new();
        // -(a + b): without parens this would re-parse as (-a) + b.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let negated = synth.unary_op(UnOp::Neg, sum);
        let Expression::UnaryOp(unary_op) = &negated else {
            panic!("expected unop");
        };
        assert!(matches!(unary_op.operand, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_compound_operand() {
        let synth = Synth::new();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let cast = synth.type_cast(sum, synth.ty_named("number"));
        let Expression::TypeCast(type_cast) = &cast else {
            panic!("expected type cast");
        };
        assert!(matches!(type_cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn type_cast_parenthesizes_cast_operands() {
        let synth = Synth::new();
        // (a :: T) :: U: `a :: T :: U` is a parse error in Luau.
        let inner = synth.type_cast(synth.name_expr("a"), synth.ty_named("T"));
        let outer = synth.type_cast(inner, synth.ty_named("U"));
        let Expression::TypeCast(cast) = &outer else {
            panic!("expected type cast");
        };
        assert!(matches!(cast.expr, Expression::Parenthesized(_)));
    }

    #[test]
    fn single_value_wraps_multi_value_forms() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![]);
        assert!(matches!(synth.single_value(call), Expression::Parenthesized(_)));
        assert!(matches!(synth.single_value(synth.vararg()), Expression::Parenthesized(_)));
        // Single-value expressions pass through unchanged.
        assert!(matches!(synth.single_value(synth.name_expr("x")), Expression::Name(_)));
        assert!(matches!(synth.single_value(synth.number_i64(1)), Expression::Number(_)));
    }

    #[test]
    fn prefix_positions_wrap_non_prefix_exprs() {
        let synth = Synth::new();
        // A string receiver must be parenthesized: ("s"):rep(2).
        let called = synth.method_call(synth.string("s"), "rep", vec![synth.number("2")]);
        assert!(matches!(chain(&called).prefix, Expression::Parenthesized(_)));

        // A table prefix must be parenthesized: ({}).x.
        let accessed = synth.field(synth.table(vec![]), "x");
        assert!(matches!(chain(&accessed).prefix, Expression::Parenthesized(_)));
    }

    #[test]
    fn a_suffix_joins_the_run_it_follows() {
        // f().x[1]:m(), one run after `f`.
        let synth = Synth::new();
        let called = synth.call(synth.name_expr("f"), vec![]);
        let indexed = synth.index(synth.field(called, "x"), synth.number("1"));
        let run = synth.method_call(indexed, "m", vec![]);
        let suffixes = &chain(&run).suffixes;
        assert!(matches!(chain(&run).prefix, Expression::Name(_)));
        assert!(matches!(
            **suffixes,
            [Suffix::Call(_), Suffix::Field(_), Suffix::Index { .. }, Suffix::Call(_)]
        ));
    }

    #[test]
    fn an_operand_bare_at_the_same_level_joins_the_run() {
        let synth = Synth::new();
        let run_len = |expr: &Expression| {
            let Expression::BinaryOp(run) = expr else {
                panic!("expected binop");
            };
            run.links.len()
        };
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
    fn field_or_index_picks_form_by_name() {
        let synth = Synth::new();
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
        // Unpinned: `goto` is a keyword somewhere, so it gets bracketed.
        let unpinned = Synth::new();
        let unpinned_goto = unpinned.field_or_index(unpinned.name_expr("t"), "goto");
        assert!(matches!(chain(&unpinned_goto).suffixes.last(), Suffix::Index { .. }));

        // Pinned to Luau: `goto` is a plain identifier there.
        let luau = Synth::new().with_version(LuaVersion::Luau);
        let luau_goto = luau.field_or_index(luau.name_expr("t"), "goto");
        assert!(matches!(chain(&luau_goto).suffixes.last(), Suffix::Field(_)));

        // Pinned to 5.4: still a keyword, still bracketed.
        let lua54 = Synth::new().with_version(LuaVersion::Lua54);
        let lua54_goto = lua54.field_or_index(lua54.name_expr("t"), "goto");
        assert!(matches!(chain(&lua54_goto).suffixes.last(), Suffix::Index { .. }));
    }

    #[test]
    fn builds_sugar_call_forms() {
        let synth = Synth::new();
        let string_call = synth.call_string(synth.name_expr("require"), "module");
        assert!(matches!(last_call(&string_call).args, FunctionArgs::StringLiteral(_)));

        let table_call = synth.call_table(
            synth.name_expr("configure"),
            vec![FieldSpec::Named("debug", synth.boolean(true))],
        );
        assert!(matches!(last_call(&table_call).args, FunctionArgs::TableConstructor(_)));

        let method_call = synth.method_call_string(synth.name_expr("s"), "gsub", "pattern");
        assert!(last_call(&method_call).method.is_some());
        assert!(matches!(last_call(&method_call).args, FunctionArgs::StringLiteral(_)));
    }

    #[test]
    fn builds_mixed_table() {
        let synth = Synth::new();
        let table = synth.table(vec![
            FieldSpec::Positional(synth.number("1")),
            FieldSpec::Named("a", synth.number("2")),
            FieldSpec::Bracketed(synth.string("k"), synth.boolean(true)),
        ]);

        let Expression::TableConstructor(constructor) = &table else {
            panic!("expected table constructor");
        };
        assert_eq!(constructor.fields.len(), 3);
        assert!(matches!(constructor.fields.items[0], Field::Positional { .. }));
        assert!(matches!(constructor.fields.items[1], Field::Named { .. }));
        assert!(matches!(constructor.fields.items[2], Field::Bracketed { .. }));
    }

    #[test]
    fn array_is_positional_and_record_brackets_unsafe_keys() {
        let synth = Synth::new();
        let Expression::TableConstructor(array) =
            synth.array(vec![synth.number("1"), synth.number("2")])
        else {
            panic!("expected table");
        };
        assert!(array.fields.iter().all(|field| matches!(field, Field::Positional { .. })));

        // Unsafe keys fall back to the bracketed form.
        let Expression::TableConstructor(record) =
            synth.record(vec![("good", synth.number("1")), ("not valid", synth.number("2"))])
        else {
            panic!("expected table");
        };
        assert!(matches!(record.fields.items[0], Field::Named { .. }));
        assert!(matches!(record.fields.items[1], Field::Bracketed { .. }));
    }

    #[test]
    fn builds_if_expr_with_elseif() {
        let synth = Synth::new();
        let expr = synth.if_expr(
            synth.boolean(true),
            synth.number("1"),
            vec![(synth.boolean(false), synth.number("2"))],
            synth.number("3"),
        );
        let Expression::IfExpression(if_expr) = &expr else {
            panic!("expected if expression");
        };
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    }

    #[test]
    fn builds_type_instantiations_and_typed_method_calls() {
        let synth = Synth::new().with_version(LuaVersion::Luau);
        let instantiated =
            synth.type_instantiation(synth.name_expr("f"), vec![synth.ty_named("integer")]);
        let Suffix::TypeInstantiation { type_args, .. } = chain(&instantiated).suffixes.last()
        else {
            panic!("expected type instantiation");
        };
        assert_eq!(type_args.args.len(), 1);

        let method = synth.method_call_with_types(
            synth.name_expr("receiver"),
            "map",
            vec![synth.ty_named("string")],
            vec![synth.string("value")],
        );
        let method_type_args =
            last_call(&method).explicit_type_args.as_ref().expect("type arguments");
        assert_eq!(method_type_args.args.len(), 1);
    }
}
