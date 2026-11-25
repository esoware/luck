use luck_allocator::Box;
use luck_token::CompoundOp;

use crate::expr::{Expression, Var};
use crate::function::FunctionAttribute;
use crate::stmt::{
    Block, ConditionalBinding, ElseClause, ElseIfClause, FunctionName, LocalAssignmentKind,
    LocalFunctionKind, Statement, TypeDeclarationValue, TypedName,
};
use crate::synth::{FunctionSignature, Synth};
use crate::types::{GenericTypeList, Type};

impl<'ast> Synth<'ast> {
    /// For attributes, generics, typed parameters, varargs, or a return
    /// type, use [`Synth::function_decl_full`].
    ///
    /// # Panics
    ///
    /// Panics if `method` or a name in `name_path` or `params` isn't a valid identifier, as [`Synth::name`] does.
    pub fn function_decl(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        params: &[&str],
        body: Block<'ast>,
    ) -> Statement<'ast> {
        let signature = FunctionSignature { params: self.params(params), ..Default::default() };
        self.function_decl_full(name_path, method, [], signature, body)
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    ///
    /// # Panics
    ///
    /// Panics if `method` or a name in `name_path` isn't a valid identifier, as [`Synth::name`] does.
    pub fn function_decl_full<Attributes>(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        let name = FunctionName::new(
            self.next_span(),
            self.list(name_path.iter().map(|part| self.name(part))),
            method.map(|method_name| self.name(method_name)),
        );
        Statement::new_function_decl(
            self.next_span(),
            self.list(attributes),
            name,
            self.function_body(signature, body),
            self.allocator,
        )
    }

    /// For attributes or a full signature, use [`Synth::local_function_full`].
    ///
    /// # Panics
    ///
    /// Panics if `name` or a name in `params` isn't a valid identifier, as [`Synth::name`] does.
    pub fn local_function(
        &self,
        name: &str,
        params: &[&str],
        body: Block<'ast>,
    ) -> Statement<'ast> {
        let signature = FunctionSignature { params: self.params(params), ..Default::default() };
        self.local_function_full(name, [], signature, body)
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn local_function_full<Attributes>(
        &self,
        name: &str,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        self.local_function_node(LocalFunctionKind::Local, name, attributes, signature, body)
    }

    /// `const function`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn const_function<Attributes>(
        &self,
        name: &str,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        self.local_function_node(LocalFunctionKind::Const, name, attributes, signature, body)
    }

    /// `export function`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn export_function<Attributes>(
        &self,
        name: &str,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        self.local_function_node(LocalFunctionKind::Export, name, attributes, signature, body)
    }

    fn local_function_node<Attributes>(
        &self,
        kind: LocalFunctionKind,
        name: &str,
        attributes: Attributes,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast>
    where
        Attributes: IntoIterator<Item = FunctionAttribute<'ast>>,
        Attributes::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        Statement::new_local_function(
            span,
            self.list(attributes),
            self.name(name),
            self.function_body(signature, body),
            span.start,
            kind,
            self.allocator,
        )
    }

    /// For type annotations, use [`Synth::local_full`].
    ///
    /// # Panics
    ///
    /// Panics if a name in `names` isn't a valid identifier, as [`Synth::name`] does.
    pub fn local<Exprs>(&self, names: &[&str], exprs: Exprs) -> Statement<'ast>
    where
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        let typed_names = names.iter().map(|name| self.typed_name(name, None));
        self.local_full(typed_names, exprs)
    }

    /// Build `names` with [`Synth::typed_name`].
    pub fn local_full<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        self.local_assignment(names, exprs, LocalAssignmentKind::Local)
    }

    /// `const names = exprs`.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    pub fn const_local<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        let initializers = exprs.into_iter();
        assert!(initializers.len() != 0, "synth: const declarations require an initializer");
        self.local_assignment(names, initializers, LocalAssignmentKind::Const)
    }

    /// `export local names = exprs`.
    pub fn export_local<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        self.local_assignment(names, exprs, LocalAssignmentKind::ExportLocal)
    }

    /// `export const names = exprs`.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    pub fn export_const<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        let initializers = exprs.into_iter();
        assert!(
            initializers.len() != 0,
            "synth: exported const declarations require an initializer"
        );
        self.local_assignment(names, initializers, LocalAssignmentKind::ExportConst)
    }

    fn local_assignment<Names, Exprs>(
        &self,
        names: Names,
        exprs: Exprs,
        kind: LocalAssignmentKind,
    ) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::new_local_assignment(
            self.next_span(),
            self.list(names),
            self.list(exprs),
            kind,
            self.allocator,
        )
    }

    /// Build targets with [`Synth::name_var`], [`Synth::index_var`], [`Synth::field_var`].
    pub fn assign<Targets, Values>(&self, targets: Targets, values: Values) -> Statement<'ast>
    where
        Targets: IntoIterator<Item = Var<'ast>>,
        Targets::IntoIter: ExactSizeIterator,
        Values: IntoIterator<Item = Expression<'ast>>,
        Values::IntoIter: ExactSizeIterator,
    {
        Statement::new_assignment(
            self.next_span(),
            self.list(targets),
            self.list(values),
            self.allocator,
        )
    }

    /// `target op= value`.
    pub fn compound_assign(
        &self,
        target: Var<'ast>,
        op: CompoundOp,
        value: Expression<'ast>,
    ) -> Statement<'ast> {
        Statement::new_compound_assignment(self.next_span(), target, op, value, self.allocator)
    }

    /// # Panics
    ///
    /// Panics if `call_expr` isn't a function call.
    #[expect(clippy::unused_self, reason = "every constructor is the synthesizer's")]
    pub fn call_stmt(&self, call_expr: Expression<'ast>) -> Statement<'ast> {
        let Expression::Suffixed(call) = call_expr else {
            panic!("synth: call_stmt requires a function-call expression, got {call_expr:?}");
        };
        assert!(
            call.is_call(),
            "synth: call_stmt requires a function-call expression, got {call:?}"
        );
        Statement::FunctionCall(call)
    }

    pub fn do_block(&self, block: Block<'ast>) -> Statement<'ast> {
        Statement::new_do_block(self.next_span(), block, self.allocator)
    }

    pub fn while_(&self, condition: Expression<'ast>, block: Block<'ast>) -> Statement<'ast> {
        Statement::new_while_loop(self.next_span(), None, condition, block, self.allocator)
    }

    pub fn repeat_(&self, block: Block<'ast>, condition: Expression<'ast>) -> Statement<'ast> {
        Statement::new_repeat_loop(self.next_span(), block, condition, self.allocator)
    }

    pub fn if_<ElseIfs>(
        &self,
        condition: Expression<'ast>,
        then_block: Block<'ast>,
        elseifs: ElseIfs,
        else_block: Option<Block<'ast>>,
    ) -> Statement<'ast>
    where
        ElseIfs: IntoIterator<Item = (Expression<'ast>, Block<'ast>)>,
        ElseIfs::IntoIter: ExactSizeIterator,
    {
        let span = self.next_span();
        let elseif_clauses = self.list(elseifs.into_iter().map(|(elseif_condition, block)| {
            ElseIfClause::new(self.next_span(), None, elseif_condition, block)
        }));
        let else_clause =
            else_block.map(|block| ElseClause::boxed(self.next_span(), block, self.allocator));
        Statement::new_if_stmt(
            span,
            None,
            condition,
            then_block,
            elseif_clauses,
            else_clause,
            self.allocator,
        )
    }

    /// The `local name` or `const name` of a condition, which goes in the
    /// `binding` of an `if`, an `elseif`, or a `while` built without one.
    pub fn conditional_binding(
        &self,
        name: TypedName<'ast>,
        is_const: bool,
    ) -> Box<'ast, ConditionalBinding<'ast>> {
        ConditionalBinding::boxed(self.next_span(), name, is_const, self.allocator)
    }

    /// Build `variable` with [`Synth::typed_name`].
    pub fn numeric_for(
        &self,
        variable: TypedName<'ast>,
        start: Expression<'ast>,
        limit: Expression<'ast>,
        step: Option<Expression<'ast>>,
        block: Block<'ast>,
    ) -> Statement<'ast> {
        Statement::new_numeric_for(
            self.next_span(),
            variable,
            start,
            limit,
            step,
            block,
            self.allocator,
        )
    }

    /// Build names with [`Synth::typed_name`].
    pub fn generic_for<Names, Exprs>(
        &self,
        names: Names,
        exprs: Exprs,
        block: Block<'ast>,
    ) -> Statement<'ast>
    where
        Names: IntoIterator<Item = TypedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::new_generic_for(
            self.next_span(),
            self.list(names),
            self.list(exprs),
            block,
            self.allocator,
        )
    }

    pub fn return_<Exprs>(&self, exprs: Exprs) -> Statement<'ast>
    where
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::new_return(self.next_span(), self.list(exprs), self.allocator)
    }

    pub fn break_(&self) -> Statement<'ast> {
        Statement::Break(self.next_span())
    }

    pub fn continue_(&self) -> Statement<'ast> {
        Statement::Continue(self.next_span())
    }

    /// A statement that has to end its block goes in a `do ... end` of its
    /// own when others follow it, so the printed block reads back.
    pub fn block<Stmts>(&self, stmts: Stmts) -> Block<'ast>
    where
        Stmts: IntoIterator<Item = Statement<'ast>>,
        Stmts::IntoIter: ExactSizeIterator,
    {
        let given = stmts.into_iter();
        let last_index = given.len().saturating_sub(1);
        let ended = given.enumerate().map(|(i, stmt)| {
            if i < last_index && stmt.has_to_end_block() {
                let block = Block::new(self.next_span(), self.list([stmt]));
                Statement::new_do_block(self.next_span(), block, self.allocator)
            } else {
                stmt
            }
        });
        Block::new(self.next_span(), self.list(ended))
    }

    /// `[export] type name<generics> = value`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn type_declaration(
        &self,
        is_exported: bool,
        name: &str,
        generics: Option<GenericTypeList<'ast>>,
        value: Type<'ast>,
    ) -> Statement<'ast> {
        Statement::new_type_declaration(
            self.next_span(),
            is_exported,
            self.name(name),
            generics.map(|list| self.boxed(list)),
            TypeDeclarationValue::Alias(value),
            self.allocator,
        )
    }

    /// `[export] type function name funcbody`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    pub fn type_function(
        &self,
        is_exported: bool,
        name: &str,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast> {
        let value =
            TypeDeclarationValue::TypeFunction(self.boxed(self.function_body(signature, body)));
        Statement::new_type_declaration(
            self.next_span(),
            is_exported,
            self.name(name),
            None,
            value,
            self.allocator,
        )
    }
}

#[cfg(test)]
mod tests {
    use luck_token::BinOp;

    use super::*;
    use crate::fixtures::Fixture;

    #[test]
    fn builds_local_with_binary_op() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.number_f64(1.0), BinOp::Add, synth.number_f64(2.0));
        let Statement::LocalAssignment(local) = synth.local(&["x"], [sum]) else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 1);
        assert!(matches!(*local.exprs, [Expression::BinaryOp(_)]));
    }

    #[test]
    fn builds_local_with_no_initializer() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let Statement::LocalAssignment(local) = synth.local(&["x", "y"], []) else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 2);
        assert!(local.exprs.is_empty());
    }

    #[test]
    fn builds_typed_local() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let optional = synth.ty_optional(synth.ty_named("number"));
        let stmt = synth.local_full([synth.typed_name("x", Some(optional))], [synth.nil()]);
        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let [first] = &*local.names else {
            panic!("expected one name");
        };
        assert!(matches!(first.type_annotation.as_deref(), Some(Type::Optional(_))));
    }

    #[test]
    fn builds_const_declarations() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.const_local([synth.typed_name("frozen", None)], [synth.number_f64(1.0)]);
        let Statement::LocalAssignment(const_local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(const_local.kind, LocalAssignmentKind::Const);

        let plain = synth.local(&["thawed"], [synth.number_f64(2.0)]);
        let Statement::LocalAssignment(plain_local) = &plain else {
            panic!("expected local assignment");
        };
        assert_eq!(plain_local.kind, LocalAssignmentKind::Local);

        let function_stmt =
            synth.const_function("pinned", [], FunctionSignature::default(), synth.block([]));
        let Statement::LocalFunction(local_function) = &function_stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    }

    #[test]
    fn builds_compound_assignment() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.compound_assign(
            synth.name_var("counter"),
            CompoundOp::Add,
            synth.number_f64(1.0),
        );
        let Statement::CompoundAssignment(compound) = &stmt else {
            panic!("expected compound assignment");
        };
        assert!(matches!(compound.var, Var::Name(_)));
        assert_eq!(compound.op, CompoundOp::Add);
    }

    #[test]
    fn builds_if_with_elseif() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.if_(
            synth.boolean(true),
            synth.block([]),
            [(synth.boolean(false), synth.block([]))],
            None,
        );
        let Statement::IfStatement(if_stmt) = &stmt else {
            panic!("expected if statement");
        };
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_none());
    }

    #[test]
    fn builds_generic_for() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let call = synth.call(synth.name_expr("pairs"), [synth.name_expr("t")]);
        let stmt = synth.generic_for(
            [synth.typed_name("k", None), synth.typed_name("v", None)],
            [call],
            synth.block([]),
        );
        let Statement::GenericFor(generic_for) = &stmt else {
            panic!("expected generic for");
        };
        assert_eq!(generic_for.names.len(), 2);
        assert_eq!(generic_for.exprs.len(), 1);
    }

    #[test]
    fn builds_typed_numeric_for() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.numeric_for(
            synth.typed_name("i", Some(synth.ty_named("number"))),
            synth.number_f64(1.0),
            synth.number_f64(10.0),
            None,
            synth.block([]),
        );
        let Statement::NumericFor(numeric_for) = &stmt else {
            panic!("expected numeric for");
        };
        assert!(numeric_for.variable.type_annotation.is_some());
    }

    #[test]
    fn builds_loop_exits() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert!(matches!(synth.continue_(), Statement::Continue(_)));
        assert!(matches!(synth.break_(), Statement::Break(_)));
    }

    #[test]
    fn a_block_ends_what_has_to_end_it_where_others_follow() {
        let fixture = Fixture::default();
        let shapes = |synth: Synth<'_>| {
            let call = || synth.call_stmt(synth.call(synth.name_expr("f"), []));
            let block = synth.block([
                synth.break_(),
                call(),
                synth.return_([]),
                call(),
                synth.continue_(),
                synth.return_([]),
            ]);
            block.stmts.iter().map(|stmt| matches!(stmt, Statement::DoBlock(_))).collect::<Vec<_>>()
        };
        assert_eq!(shapes(fixture.synth()), [true, false, true, false, true, false]);
    }

    #[test]
    fn builds_function_declaration_with_method() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt = synth.function_decl(&["a", "b"], Some("greet"), &["self"], synth.block([]));
        let Statement::FunctionDecl(function_decl) = &stmt else {
            panic!("expected function declaration");
        };
        let names: Vec<&str> =
            function_decl.name.names.iter().map(|name| name.text.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(function_decl.name.method.map(|method| method.text.as_str()), Some("greet"));
    }

    #[test]
    fn builds_type_declaration_with_generics() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let type_params = synth.generic_type_list([("T", false), ("Rest", true)]);
        let stmt = synth.type_declaration(true, "Alias", Some(type_params), synth.ty_named("T"));
        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(declaration.is_exported);
        assert!(matches!(declaration.value, TypeDeclarationValue::Alias(_)));
        let generics = declaration.generics.as_ref().expect("generics present");
        let packs: Vec<bool> = generics.params.iter().map(|param| param.is_pack).collect();
        assert_eq!(packs, [false, true]);
    }

    #[test]
    fn builds_type_function() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let signature = FunctionSignature {
            params: vec![synth.typed_name("ty", None)],
            ..FunctionSignature::default()
        };
        let stmt = synth.type_function(false, "Partial", signature, synth.block([]));
        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(!declaration.is_exported);
        assert!(matches!(declaration.value, TypeDeclarationValue::TypeFunction(_)));
    }

    #[test]
    fn builds_exported_locals() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let exported = synth.export_local(
            [synth.typed_name("value", Some(synth.ty_named("number")))],
            [synth.number_f64(1.0)],
        );
        assert!(matches!(
            exported,
            Statement::LocalAssignment(local) if local.kind == LocalAssignmentKind::ExportLocal
        ));
    }
}
