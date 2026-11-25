use std::mem;

use luck_allocator::Box;
use luck_token::{Associativity, BinOp, UnOp};

use crate::expr::{
    BinaryLink, BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, Suffix,
    SuffixedExpression, TableConstructor, Var,
};
use crate::function::FunctionAttribute;
use crate::identifier::is_valid_identifier;
use crate::one_or_more::OneOrMore;
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

impl<'ast> Synth<'ast> {
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn name_var(&self, name: &str) -> Var<'ast> {
        Var::Name(self.boxed(self.name(name)))
    }

    pub fn index_var(&self, prefix: Expression<'ast>, index: Expression<'ast>) -> Var<'ast> {
        Var::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn field_var(&self, prefix: Expression<'ast>, name: &str) -> Var<'ast> {
        Var::Suffixed(self.suffixed(prefix, Suffix::Field(self.name(name))))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn name_expr(&self, name: &str) -> Expression<'ast> {
        Expression::Name(self.boxed(self.name(name)))
    }

    pub fn index(&self, prefix: Expression<'ast>, index: Expression<'ast>) -> Expression<'ast> {
        Expression::Suffixed(self.index_chain(prefix, index))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
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
        if let Expression::Suffixed(mut chain) = prefix {
            chain.span = self.next_span();
            chain.suffixes.push(suffix, self.allocator);
            return chain;
        }
        let suffixes = OneOrMore::new(suffix, self.allocator);
        SuffixedExpression::boxed(self.next_span(), prefix, suffixes, self.allocator)
    }

    /// `prefix.name` when `name` is an identifier, `prefix["name"]`
    /// otherwise.
    pub fn field_or_index(&self, prefix: Expression<'ast>, name: &str) -> Expression<'ast> {
        if is_valid_identifier(name) {
            self.field(prefix, name)
        } else {
            let key = self.string(name);
            self.index(prefix, key)
        }
    }

    /// `lhs op rhs`. An operand that's a run at `op`'s level joins the run
    /// where it groups as this one would, first under a left-associative
    /// `op` and last under a right-associative one, so `(a + b) - c` builds
    /// one run, and `a - (b - c)` a run in a run.
    pub fn binary_op(
        &self,
        lhs: Expression<'ast>,
        op: BinOp,
        rhs: Expression<'ast>,
    ) -> Expression<'ast> {
        let span = self.next_span();
        let (level, associativity) = op.precedence();
        let run = match (associativity, lhs, rhs) {
            (Associativity::Left, Expression::BinaryOp(mut run), operand)
                if run.precedence().0 == level =>
            {
                run.span = span;
                run.links.push(BinaryLink::new(op, operand), self.allocator);
                run
            }
            (Associativity::Right, first, Expression::BinaryOp(mut run))
                if run.precedence().0 == level =>
            {
                let second = mem::replace(&mut run.first, first);
                run.span = span;
                run.links.insert(0, BinaryLink::new(op, second), self.allocator);
                run
            }
            (Associativity::Left | Associativity::Right, first, operand) => {
                let links = OneOrMore::new(BinaryLink::new(op, operand), self.allocator);
                BinaryOp::boxed(span, first, links, self.allocator)
            }
        };
        Expression::BinaryOp(run)
    }

    pub fn unary_op(&self, op: UnOp, operand: Expression<'ast>) -> Expression<'ast> {
        Expression::new_unary_op(self.next_span(), op, operand, self.allocator)
    }

    /// Truncates a call or `...`, the only multi-value forms, to one value by
    /// parenthesizing it. Anything else passes through unchanged.
    ///
    /// Use it at the end of a call, return, or assignment list when exactly
    /// one value is meant, such as a bytecode `CALL` that requests one result.
    pub fn single_value(&self, expr: Expression<'ast>) -> Expression<'ast> {
        if expr.is_multi_value() {
            Expression::new_parenthesized(self.next_span(), expr, self.allocator)
        } else {
            expr
        }
    }

    /// A type cast: `expr :: T`.
    pub fn type_cast(&self, expr: Expression<'ast>, ty: Type<'ast>) -> Expression<'ast> {
        Expression::new_type_cast(self.next_span(), expr, ty, self.allocator)
    }

    /// `expr<<T, U>>`.
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

    /// `receiver:name<<T, U>>(args)`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
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

    /// `callee"content"`, the string argument form, whose value is `content`.
    pub fn call_string(&self, callee: Expression<'ast>, content: &str) -> Expression<'ast> {
        let args = FunctionArgs::StringLiteral(self.boxed(self.string_literal(content.as_bytes())));
        Expression::Suffixed(self.suffixed(callee, Suffix::Call(args)))
    }

    /// # Panics
    ///
    /// Panics if `name` or a [`FieldSpec::Named`] key isn't a valid identifier, as [`Synth::name`] does.
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
    pub fn method_call_string(
        &self,
        receiver: Expression<'ast>,
        name: &str,
        content: &str,
    ) -> Expression<'ast> {
        let args = FunctionArgs::StringLiteral(self.boxed(self.string_literal(content.as_bytes())));
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
    pub fn table<'name, Fields>(&self, fields: Fields) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = FieldSpec<'name, 'ast>>,
        Fields::IntoIter: ExactSizeIterator,
    {
        Expression::TableConstructor(self.table_constructor(fields))
    }

    pub fn array<Values>(&self, values: Values) -> Expression<'ast>
    where
        Values: IntoIterator<Item = Expression<'ast>>,
        Values::IntoIter: ExactSizeIterator,
    {
        self.table(values.into_iter().map(FieldSpec::Positional))
    }

    /// Keys that aren't safe identifiers fall back to the bracketed string form.
    pub fn record<'name, Fields>(&self, fields: Fields) -> Expression<'ast>
    where
        Fields: IntoIterator<Item = (&'name str, Expression<'ast>)>,
        Fields::IntoIter: ExactSizeIterator,
    {
        self.table(fields.into_iter().map(|(name, value)| {
            if is_valid_identifier(name) {
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
    pub fn function_def(&self, params: &[&str], body: Block<'ast>) -> Expression<'ast> {
        let signature = FunctionSignature { params: self.params(params), ..Default::default() };
        self.function_def_full([], signature, body)
    }

    /// Build `attributes` with [`Synth::function_attribute`].
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

    /// `if condition then then_expr elseif ... else else_expr`.
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
    fn binary_op_nests_an_operand_at_another_level() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        for (inner, outer) in [(BinOp::Add, BinOp::Mul), (BinOp::Mul, BinOp::Add)] {
            let nested = synth.binary_op(synth.name_expr("a"), inner, synth.name_expr("b"));
            let run_expr = synth.binary_op(nested, outer, synth.name_expr("c"));
            assert_eq!(run(&run_expr).links.len(), 1, "{inner:?} under {outer:?}");
            assert!(matches!(run(&run_expr).first, Expression::BinaryOp(_)));
        }
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
        assert!(matches!(synth.single_value(synth.number_f64(1.0)), Expression::Number(_)));
    }

    #[test]
    fn a_suffix_joins_the_run_it_follows() {
        // f().x[1]:m(), one run after `f`.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let called = synth.call(synth.name_expr("f"), []);
        let indexed = synth.index(synth.field(called, "x"), synth.number_f64(1.0));
        let method_run = synth.method_call(indexed, "m", []);
        assert!(matches!(chain(&method_run).prefix, Expression::Name(_)));
        assert!(matches!(
            *chain(&method_run).suffixes,
            [Suffix::Call(_), Suffix::Field(_), Suffix::Index { .. }, Suffix::MethodCall(_)]
        ));
    }

    #[test]
    fn an_operand_at_the_same_level_joins_the_run_where_it_groups_the_same() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let run_len = |expr: &Expression<'_>| run(expr).links.len();
        // (a + b) - c is a + b - c.
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        assert_eq!(run_len(&synth.binary_op(sum, BinOp::Sub, synth.name_expr("c"))), 2);
        // So is a .. (b .. c) a .. b .. c.
        let concat = synth.binary_op(synth.name_expr("b"), BinOp::Concat, synth.name_expr("c"));
        assert_eq!(run_len(&synth.binary_op(synth.name_expr("a"), BinOp::Concat, concat)), 2);
        // a - (b - c) and (a .. b) .. c each keep a run of their own.
        let difference = synth.binary_op(synth.name_expr("b"), BinOp::Sub, synth.name_expr("c"));
        assert_eq!(run_len(&synth.binary_op(synth.name_expr("a"), BinOp::Sub, difference)), 1);
        let head = synth.binary_op(synth.name_expr("a"), BinOp::Concat, synth.name_expr("b"));
        assert_eq!(run_len(&synth.binary_op(head, BinOp::Concat, synth.name_expr("c"))), 1);
    }

    #[test]
    fn a_run_extended_a_link_at_a_time_keeps_its_order() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let names: Vec<String> = (0..2_000).map(|i| format!("v{i}")).collect();
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
        for _ in 0..10_000 {
            sum = synth.binary_op(sum, BinOp::Add, synth.nil());
        }
        assert_eq!(run(&sum).links.len(), 10_000);
        // The links' doubling allocations and the run's box, well under the
        // quadratic space that copying the run at each link would take.
        let used = fixture.allocator().used_bytes() - one_link;
        assert!(used < 10_000 * 4 * size_of::<BinaryLink<'_>>(), "{used} bytes");
    }

    #[test]
    fn field_or_index_picks_form_by_name() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let valid = synth.field_or_index(synth.name_expr("t"), "valid_name");
        assert!(matches!(chain(&valid).suffixes.last(), Suffix::Field(_)));
        let goto = synth.field_or_index(synth.name_expr("t"), "goto");
        assert!(matches!(chain(&goto).suffixes.last(), Suffix::Field(_)));
        for bad in ["not an ident", "1st", "", "end"] {
            let bracketed = synth.field_or_index(synth.name_expr("t"), bad);
            assert!(
                matches!(chain(&bracketed).suffixes.last(), Suffix::Index { .. }),
                "{bad:?} should use the bracketed form"
            );
        }
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
            FieldSpec::Positional(synth.number_f64(1.0)),
            FieldSpec::Named("a", synth.number_f64(2.0)),
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
            synth.array([synth.number_f64(1.0), synth.number_f64(2.0)])
        else {
            panic!("expected table");
        };
        assert!(array.fields.iter().all(|field| matches!(field, Field::Positional(_))));

        // Unsafe keys fall back to the bracketed form.
        let Expression::TableConstructor(record) =
            synth.record([("good", synth.number_f64(1.0)), ("not valid", synth.number_f64(2.0))])
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
            synth.number_f64(1.0),
            [(synth.boolean(false), synth.number_f64(2.0))],
            synth.number_f64(3.0),
        );
        let Expression::IfExpression(if_expr) = &expr else {
            panic!("expected if expression");
        };
        assert_eq!(if_expr.elseif_clauses.len(), 1);
    }

    #[test]
    fn builds_type_instantiations_and_typed_method_calls() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
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
