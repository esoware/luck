use luck_token::CompoundOp;

use crate::expr::{Expression, Var};
use crate::function::{FunctionAttribute, Parameter};
use crate::stmt::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalAssignmentKind, LocalFunction, LocalFunctionKind, NumericFor, RepeatLoop, ReturnStatement,
    Statement, TypeDeclaration, TypeDeclarationValue, WhileLoop,
};
use crate::synth::{FunctionSignature, Synth};
use crate::types::{GenericTypeList, Type};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl<'ast> Synth<'ast> {
    /// For attributes, generics, typed parameters, varargs, or a return
    /// type, use [`Synth::function_decl_full`].
    ///
    /// # Panics
    ///
    /// Panics if `method` or a name in `name_path` or `params` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
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
    #[must_use]
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
        let name = FunctionName {
            span: self.next_span(),
            names: self.list(name_path.iter().map(|part| self.name(part))),
            method: method.map(|method_name| self.name(method_name)),
        };
        Statement::FunctionDecl(self.boxed(FunctionDecl {
            span: self.next_span(),
            attributes: self.list(attributes),
            name,
            body: self.function_body(signature, body),
        }))
    }

    /// For attributes or a full signature, use [`Synth::local_function_full`].
    ///
    /// # Panics
    ///
    /// Panics if `name` or a name in `params` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
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
    #[must_use]
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

    /// Luau: `const function`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
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

    /// Luau: `export function`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
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
        Statement::LocalFunction(self.boxed(LocalFunction {
            span: self.next_span(),
            attributes: self.list(attributes),
            name: self.name(name),
            body: self.function_body(signature, body),
            kind,
        }))
    }

    /// Lua 5.5: `global function name`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn global_function(
        &self,
        name: &str,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast> {
        Statement::GlobalFunction(self.boxed(GlobalFunction {
            span: self.next_span(),
            name: self.name(name),
            body: self.function_body(signature, body),
        }))
    }

    /// For type annotations or Lua 5.4 attributes, use [`Synth::local_full`].
    ///
    /// # Panics
    ///
    /// Panics if a name in `names` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn local<Exprs>(&self, names: &[&str], exprs: Exprs) -> Statement<'ast>
    where
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        let attributed = names.iter().map(|name| self.attributed_name(name, None, None));
        self.local_full(attributed, exprs)
    }

    /// Build `names` with [`Synth::attributed_name`].
    #[must_use]
    pub fn local_full<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = AttributedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        self.local_assignment(names, exprs, LocalAssignmentKind::Local)
    }

    /// Luau: `const names = exprs`.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn const_local<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = AttributedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        let initializers = exprs.into_iter();
        assert!(initializers.len() != 0, "synth: const declarations require an initializer");
        self.local_assignment(names, initializers, LocalAssignmentKind::Const)
    }

    /// Luau: `export local names = exprs`.
    #[must_use]
    pub fn export_local<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = AttributedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        self.local_assignment(names, exprs, LocalAssignmentKind::ExportLocal)
    }

    /// Luau: `export const names = exprs`.
    ///
    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn export_const<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = AttributedName<'ast>>,
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
        Names: IntoIterator<Item = AttributedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::LocalAssignment(self.boxed(LocalAssignment {
            span: self.next_span(),
            names: self.list(names),
            exprs: self.list(exprs),
            kind,
        }))
    }

    /// One declared name for [`Synth::local_full`] / [`Synth::global_declaration`],
    /// with an optional Luau type annotation or Lua 5.4+ attribute (`"const"`
    /// or `"close"`). No dialect has both.
    ///
    /// # Panics
    ///
    /// Panics if `name` or `attribute` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn attributed_name(
        &self,
        name: &str,
        ty: Option<Type<'ast>>,
        attribute: Option<&str>,
    ) -> AttributedName<'ast> {
        AttributedName {
            name: self.name(name),
            type_annotation: ty,
            attribute: attribute.map(|attribute_name| self.boxed(self.attribute(attribute_name))),
        }
    }

    fn attribute(&self, name: &str) -> Attribute<'ast> {
        Attribute { span: self.next_span(), name: self.name(name) }
    }

    /// Lua 5.5: `global names = exprs`, or with no `=` when `exprs` is empty.
    #[must_use]
    pub fn global_declaration<Names, Exprs>(&self, names: Names, exprs: Exprs) -> Statement<'ast>
    where
        Names: IntoIterator<Item = AttributedName<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::GlobalDeclaration(self.boxed(GlobalDeclaration {
            span: self.next_span(),
            names: self.list(names),
            exprs: self.list(exprs),
        }))
    }

    /// Lua 5.5: `global *`, or `global <attribute> *`.
    ///
    /// # Panics
    ///
    /// Panics if `attribute` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn global_star(&self, attribute: Option<&str>) -> Statement<'ast> {
        Statement::GlobalStar(self.boxed(GlobalStar {
            span: self.next_span(),
            attribute: attribute.map(|name| self.attribute(name)),
        }))
    }

    /// Build targets with [`Synth::name_var`], [`Synth::index_var`], [`Synth::field_var`].
    #[must_use]
    pub fn assign<Targets, Values>(&self, targets: Targets, values: Values) -> Statement<'ast>
    where
        Targets: IntoIterator<Item = Var<'ast>>,
        Targets::IntoIter: ExactSizeIterator,
        Values: IntoIterator<Item = Expression<'ast>>,
        Values::IntoIter: ExactSizeIterator,
    {
        Statement::Assignment(self.boxed(Assignment {
            span: self.next_span(),
            targets: self.list(targets),
            values: self.list(values),
        }))
    }

    /// Luau: `target op= value`.
    #[must_use]
    pub fn compound_assign(
        &self,
        target: Var<'ast>,
        op: CompoundOp,
        value: Expression<'ast>,
    ) -> Statement<'ast> {
        Statement::CompoundAssignment(self.boxed(CompoundAssignment {
            span: self.next_span(),
            var: target,
            op,
            expr: value,
        }))
    }

    /// # Panics
    ///
    /// Panics if `call_expr` isn't a function call.
    #[must_use]
    #[expect(clippy::panic, reason = "documented panic")]
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

    #[must_use]
    pub fn do_block(&self, block: Block<'ast>) -> Statement<'ast> {
        Statement::DoBlock(self.boxed(DoBlock { span: self.next_span(), block }))
    }

    #[must_use]
    pub fn while_(&self, condition: Expression<'ast>, block: Block<'ast>) -> Statement<'ast> {
        Statement::WhileLoop(self.boxed(WhileLoop { span: self.next_span(), condition, block }))
    }

    #[must_use]
    pub fn repeat_(&self, block: Block<'ast>, condition: Expression<'ast>) -> Statement<'ast> {
        Statement::RepeatLoop(self.boxed(RepeatLoop { span: self.next_span(), block, condition }))
    }

    #[must_use]
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
            ElseIfClause { span: self.next_span(), condition: elseif_condition, block }
        }));
        let else_clause = else_block.map(|block| ElseClause { span: self.next_span(), block });
        Statement::IfStatement(self.boxed(IfStatement {
            span,
            condition,
            block: then_block,
            elseif_clauses,
            else_clause,
        }))
    }

    /// Build `variable` with [`Synth::param`] or [`Synth::param_typed`].
    #[must_use]
    pub fn numeric_for(
        &self,
        variable: Parameter<'ast>,
        start: Expression<'ast>,
        limit: Expression<'ast>,
        step: Option<Expression<'ast>>,
        block: Block<'ast>,
    ) -> Statement<'ast> {
        Statement::NumericFor(self.boxed(NumericFor {
            span: self.next_span(),
            name: variable.name,
            type_annotation: variable.type_annotation,
            start,
            limit,
            step,
            block,
        }))
    }

    /// Build names with [`Synth::param`] or [`Synth::param_typed`].
    #[must_use]
    pub fn generic_for<Names, Exprs>(
        &self,
        names: Names,
        exprs: Exprs,
        block: Block<'ast>,
    ) -> Statement<'ast>
    where
        Names: IntoIterator<Item = Parameter<'ast>>,
        Names::IntoIter: ExactSizeIterator,
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        Statement::GenericFor(self.boxed(GenericFor {
            span: self.next_span(),
            names: self.list(names),
            exprs: self.list(exprs),
            block,
        }))
    }

    /// # Panics
    ///
    /// Panics if `label` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn goto_(&self, label: &str) -> Statement<'ast> {
        Statement::Goto(
            self.boxed(GotoStatement { span: self.next_span(), name: self.name(label) }),
        )
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn label(&self, name: &str) -> Statement<'ast> {
        Statement::Label(
            self.boxed(LabelStatement { span: self.next_span(), name: self.name(name) }),
        )
    }

    /// Lua 5.1 and Luau only allow `break` as the last statement, so use [`Synth::break_`] there.
    #[must_use]
    pub fn break_stmt(&self) -> Statement<'ast> {
        Statement::Break(self.next_span())
    }

    /// The formatter drops these; only compact output prints them.
    #[must_use]
    pub fn empty_stmt(&self) -> Statement<'ast> {
        Statement::EmptyStatement(self.next_span())
    }

    #[must_use]
    pub fn return_<Exprs>(&self, exprs: Exprs) -> LastStatement<'ast>
    where
        Exprs: IntoIterator<Item = Expression<'ast>>,
        Exprs::IntoIter: ExactSizeIterator,
    {
        LastStatement::Return(
            self.boxed(ReturnStatement { span: self.next_span(), exprs: self.list(exprs) }),
        )
    }

    #[must_use]
    pub fn break_(&self) -> LastStatement<'ast> {
        LastStatement::Break(self.next_span())
    }

    /// Luau: `continue`.
    #[must_use]
    pub fn continue_(&self) -> LastStatement<'ast> {
        LastStatement::Continue(self.next_span())
    }

    #[must_use]
    pub fn block<Stmts>(&self, stmts: Stmts, last: Option<LastStatement<'ast>>) -> Block<'ast>
    where
        Stmts: IntoIterator<Item = Statement<'ast>>,
        Stmts::IntoIter: ExactSizeIterator,
    {
        Block { span: self.next_span(), stmts: self.list(stmts), last_stmt: last }
    }

    /// Luau: `[export] type name<generics> = value`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn type_declaration(
        &self,
        is_exported: bool,
        name: &str,
        generics: Option<GenericTypeList<'ast>>,
        value: Type<'ast>,
    ) -> Statement<'ast> {
        Statement::TypeDeclaration(self.boxed(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.name(name),
            generics: generics.map(|list| self.boxed(list)),
            value: TypeDeclarationValue::Alias(value),
        }))
    }

    /// Luau: `[export] type function name funcbody`.
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::name`] does.
    #[must_use]
    pub fn type_function(
        &self,
        is_exported: bool,
        name: &str,
        signature: FunctionSignature<'ast>,
        body: Block<'ast>,
    ) -> Statement<'ast> {
        Statement::TypeDeclaration(self.boxed(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.name(name),
            generics: None,
            value: TypeDeclarationValue::TypeFunction(
                self.boxed(self.function_body(signature, body)),
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{BinOp, LuaVersion};

    use super::*;
    use crate::fixtures::Fixture;

    #[test]
    fn builds_local_with_binary_op() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("2"));
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
        let stmt =
            synth.local_full([synth.attributed_name("x", Some(optional), None)], [synth.nil()]);
        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let [first] = &*local.names else {
            panic!("expected one name");
        };
        assert!(matches!(first.type_annotation, Some(Type::Optional(_))));
    }

    #[test]
    fn builds_const_declarations() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt =
            synth.const_local([synth.attributed_name("frozen", None, None)], [synth.number_i64(1)]);
        let Statement::LocalAssignment(const_local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(const_local.kind, LocalAssignmentKind::Const);

        let plain = synth.local(&["thawed"], [synth.number_i64(2)]);
        let Statement::LocalAssignment(plain_local) = &plain else {
            panic!("expected local assignment");
        };
        assert_eq!(plain_local.kind, LocalAssignmentKind::Local);

        let function_stmt =
            synth.const_function("pinned", [], FunctionSignature::default(), synth.block([], None));
        let Statement::LocalFunction(local_function) = &function_stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    }

    #[test]
    fn builds_globals() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt =
            synth.global_declaration([synth.attributed_name("shared", None, Some("const"))], []);
        let Statement::GlobalDeclaration(declaration) = &stmt else {
            panic!("expected global declaration");
        };
        assert!(declaration.names.iter().all(|name| name.attribute.is_some()));

        let function =
            synth.global_function("main", FunctionSignature::default(), synth.block([], None));
        assert!(matches!(function, Statement::GlobalFunction(_)));

        let Statement::GlobalStar(star) = synth.global_star(Some("const")) else {
            panic!("expected global star");
        };
        assert!(star.attribute.is_some());
    }

    #[test]
    fn builds_compound_assignment() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt =
            synth.compound_assign(synth.name_var("counter"), CompoundOp::Add, synth.number("1"));
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
            synth.block([], None),
            [(synth.boolean(false), synth.block([], None))],
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
        let stmt =
            synth.generic_for([synth.param("k"), synth.param("v")], [call], synth.block([], None));
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
            synth.param_typed("i", synth.ty_named("number")),
            synth.number("1"),
            synth.number("10"),
            None,
            synth.block([], None),
        );
        let Statement::NumericFor(numeric_for) = &stmt else {
            panic!("expected numeric for");
        };
        assert!(numeric_for.type_annotation.is_some());
    }

    #[test]
    fn builds_goto_and_label() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert!(matches!(synth.goto_("done"), Statement::Goto(_)));
        assert!(matches!(synth.label("done"), Statement::Label(_)));
    }

    #[test]
    fn builds_continue_and_breaks() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert!(matches!(synth.continue_(), LastStatement::Continue(_)));
        assert!(matches!(synth.break_(), LastStatement::Break(_)));
        assert!(matches!(synth.break_stmt(), Statement::Break(_)));
        assert!(matches!(synth.empty_stmt(), Statement::EmptyStatement(_)));
    }

    #[test]
    fn builds_function_declaration_with_method() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let stmt =
            synth.function_decl(&["a", "b"], Some("greet"), &["self"], synth.block([], None));
        let Statement::FunctionDecl(function_decl) = &stmt else {
            panic!("expected function declaration");
        };
        let names: Vec<&str> = function_decl.name.names.iter().map(|name| name.text).collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(function_decl.name.method.map(|method| method.text), Some("greet"));
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
        let signature =
            FunctionSignature { params: vec![synth.param("ty")], ..FunctionSignature::default() };
        let stmt = synth.type_function(false, "Partial", signature, synth.block([], None));
        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(!declaration.is_exported);
        assert!(matches!(declaration.value, TypeDeclarationValue::TypeFunction(_)));
    }

    #[test]
    fn builds_exported_locals() {
        let fixture = Fixture::default();
        let synth = fixture.synth().with_version(LuaVersion::Luau);
        let exported = synth.export_local(
            [synth.attributed_name("value", Some(synth.ty_named("integer")), None)],
            [synth.integer_i64(1)],
        );
        assert!(matches!(
            exported,
            Statement::LocalAssignment(local) if local.kind == LocalAssignmentKind::ExportLocal
        ));
    }
}
