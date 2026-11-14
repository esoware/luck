use luck_token::{CompoundOp, Token};

use crate::expr::{Expression, Var};
use crate::function::{FunctionAttribute, Parameter};
use crate::punctuated::Punctuated;
use crate::stmt::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, FunctionDecl, FunctionName, GenericFor, GlobalDeclaration, GlobalFunction,
    GlobalStar, GotoStatement, IfStatement, LabelStatement, LastStatement, LocalAssignment,
    LocalFunction, LocalFunctionKind, NumericFor, RepeatLoop, ReturnStatement, Statement,
    TypeDeclaration, TypeDeclarationValue, WhileLoop,
};
use crate::synth::{FunctionSignature, Synth};
use crate::types::{GenericTypeList, Type};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the synthesizer is one type with its constructors split by node family"
)]
impl Synth {
    /// For attributes, generics, typed parameters, varargs, or a return
    /// type, use [`Synth::function_decl_full`].
    ///
    /// # Panics
    ///
    /// Panics if `method` or a name in `name_path` or `params` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn function_decl(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        params: &[&str],
        body: Block,
    ) -> Statement {
        self.function_decl_full(
            name_path,
            method,
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    ///
    /// # Panics
    ///
    /// Panics if `method` or a name in `name_path` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn function_decl_full(
        &self,
        name_path: &[&str],
        method: Option<&str>,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::FunctionDecl(Box::new(FunctionDecl {
            span: self.next_span(),
            attributes,
            name: self.function_name(name_path, method),
            body: self.function_body(signature, body),
        }))
    }

    /// For attributes or a full signature, use [`Synth::local_function_full`].
    ///
    /// # Panics
    ///
    /// Panics if `name` or a name in `params` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn local_function(&self, name: &str, params: &[&str], body: Block) -> Statement {
        self.local_function_full(
            name,
            Vec::new(),
            FunctionSignature { params: self.params(params), ..FunctionSignature::default() },
            body,
        )
    }

    /// Build `attributes` with [`Synth::function_attribute`].
    ///
    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn local_function_full(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(self.local_function_node(
            LocalFunctionKind::Local,
            name,
            attributes,
            signature,
            body,
        )))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn const_function(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(self.local_function_node(
            LocalFunctionKind::Const,
            name,
            attributes,
            signature,
            body,
        )))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn export_function(
        &self,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::LocalFunction(Box::new(self.local_function_node(
            LocalFunctionKind::Export,
            name,
            attributes,
            signature,
            body,
        )))
    }

    fn local_function_node(
        &self,
        kind: LocalFunctionKind,
        name: &str,
        attributes: Vec<FunctionAttribute>,
        signature: FunctionSignature,
        body: Block,
    ) -> LocalFunction {
        LocalFunction {
            span: self.next_span(),
            attributes,
            name: self.identifier(name),
            body: self.function_body(signature, body),
            kind,
        }
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn global_function(
        &self,
        name: &str,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::GlobalFunction(Box::new(GlobalFunction {
            span: self.next_span(),
            name: self.identifier(name),
            body: self.function_body(signature, body),
        }))
    }

    fn function_name(&self, name_path: &[&str], method: Option<&str>) -> FunctionName {
        let names: Vec<Token> = name_path.iter().map(|part| self.identifier(part)).collect();
        FunctionName {
            span: self.next_span(),
            names,
            method: method.map(|name| self.identifier(name)),
        }
    }

    /// For type annotations or Lua 5.4 attributes, use [`Synth::local_full`].
    ///
    /// # Panics
    ///
    /// Panics if a name in `names` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn local(&self, names: &[&str], exprs: Vec<Expression>) -> Statement {
        self.local_full(
            names.iter().map(|name| self.attributed_name(name, None, None)).collect(),
            exprs,
        )
    }

    #[must_use]
    pub fn local_full(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        Statement::LocalAssignment(Box::new(self.local_assignment(names, exprs)))
    }

    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn const_local(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        assert!(!exprs.is_empty(), "synth: const declarations require an initializer");
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_const: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    #[must_use]
    pub fn export_local(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_exported: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    /// # Panics
    ///
    /// Panics if `exprs` is empty; the grammar requires an initializer.
    #[must_use]
    pub fn export_const(&self, names: Vec<AttributedName>, exprs: Vec<Expression>) -> Statement {
        assert!(!exprs.is_empty(), "synth: exported const declarations require an initializer");
        Statement::LocalAssignment(Box::new(LocalAssignment {
            is_const: true,
            is_exported: true,
            ..self.local_assignment(names, exprs)
        }))
    }

    fn local_assignment(
        &self,
        names: Vec<AttributedName>,
        exprs: Vec<Expression>,
    ) -> LocalAssignment {
        LocalAssignment {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: if exprs.is_empty() { None } else { Some(Punctuated::from_items(exprs)) },
            is_const: false,
            is_exported: false,
        }
    }

    /// One declared name for [`Synth::local_full`] / [`Synth::global_declaration`],
    /// with an optional Luau type annotation or Lua 5.4+ attribute (`"const"`
    /// or `"close"`). No dialect has both.
    ///
    /// # Panics
    ///
    /// Panics if `name` or `attribute` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn attributed_name(
        &self,
        name: &str,
        ty: Option<Type>,
        attribute: Option<&str>,
    ) -> AttributedName {
        AttributedName {
            name: self.identifier(name),
            type_annotation: ty,
            attribute: attribute.map(|attribute_name| {
                Box::new(Attribute {
                    span: self.next_span(),
                    name: self.identifier(attribute_name),
                })
            }),
        }
    }

    #[must_use]
    pub fn global_declaration(
        &self,
        names: Vec<AttributedName>,
        exprs: Vec<Expression>,
    ) -> Statement {
        Statement::GlobalDeclaration(Box::new(GlobalDeclaration {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: if exprs.is_empty() { None } else { Some(Punctuated::from_items(exprs)) },
        }))
    }

    /// # Panics
    ///
    /// Panics if `attribute` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn global_star(&self, attribute: Option<&str>) -> Statement {
        Statement::GlobalStar(Box::new(GlobalStar {
            span: self.next_span(),
            attribute: attribute
                .map(|name| Attribute { span: self.next_span(), name: self.identifier(name) }),
        }))
    }

    /// Build targets with [`Synth::name_var`], [`Synth::index_var`], [`Synth::field_var`].
    #[must_use]
    pub fn assign(&self, targets: Vec<Var>, values: Vec<Expression>) -> Statement {
        Statement::Assignment(Box::new(Assignment {
            span: self.next_span(),
            targets: Punctuated::from_items(targets),
            values: Punctuated::from_items(values),
        }))
    }

    #[must_use]
    pub fn compound_assign(&self, target: Var, op: CompoundOp, value: Expression) -> Statement {
        Statement::CompoundAssignment(Box::new(CompoundAssignment {
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
    pub fn call_stmt(&self, call_expr: Expression) -> Statement {
        let Expression::Suffixed(mut call) = call_expr else {
            panic!("synth: call_stmt requires a function-call expression, got {call_expr:?}");
        };
        assert!(
            call.is_call(),
            "synth: call_stmt requires a function-call expression, got {call:?}"
        );
        // The statement takes the call's span, so give it a fresh one: two
        // statements wrapping clones of one call need distinct comment anchors.
        call.span = self.next_span();
        Statement::FunctionCall(call)
    }

    #[must_use]
    pub fn do_block(&self, block: Block) -> Statement {
        Statement::DoBlock(Box::new(DoBlock { span: self.next_span(), block }))
    }

    #[must_use]
    pub fn while_(&self, condition: Expression, block: Block) -> Statement {
        Statement::WhileLoop(Box::new(WhileLoop { span: self.next_span(), condition, block }))
    }

    #[must_use]
    pub fn repeat_(&self, block: Block, condition: Expression) -> Statement {
        Statement::RepeatLoop(Box::new(RepeatLoop { span: self.next_span(), block, condition }))
    }

    #[must_use]
    pub fn if_(
        &self,
        condition: Expression,
        then_block: Block,
        elseifs: Vec<(Expression, Block)>,
        else_block: Option<Block>,
    ) -> Statement {
        let span = self.next_span();
        let elseif_clauses: Vec<ElseIfClause> = elseifs
            .into_iter()
            .map(|(elseif_condition, block)| ElseIfClause {
                span: self.next_span(),
                condition: elseif_condition,
                block,
            })
            .collect();
        let else_clause = else_block.map(|block| ElseClause { span: self.next_span(), block });
        Statement::IfStatement(Box::new(IfStatement {
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
        variable: Parameter,
        start: Expression,
        limit: Expression,
        step: Option<Expression>,
        block: Block,
    ) -> Statement {
        Statement::NumericFor(Box::new(NumericFor {
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
    pub fn generic_for(
        &self,
        names: Vec<Parameter>,
        exprs: Vec<Expression>,
        block: Block,
    ) -> Statement {
        Statement::GenericFor(Box::new(GenericFor {
            span: self.next_span(),
            names: Punctuated::from_items(names),
            exprs: Punctuated::from_items(exprs),
            block,
        }))
    }

    /// # Panics
    ///
    /// Panics if `label` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn goto_(&self, label: &str) -> Statement {
        Statement::Goto(Box::new(GotoStatement {
            span: self.next_span(),
            name: self.identifier(label),
        }))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn label(&self, name: &str) -> Statement {
        Statement::Label(Box::new(LabelStatement {
            span: self.next_span(),
            name: self.identifier(name),
        }))
    }

    /// Lua 5.1 and Luau only allow `break` as the last statement, so use [`Synth::break_`] there.
    #[must_use]
    pub fn break_stmt(&self) -> Statement {
        Statement::Break(self.next_span())
    }

    /// The formatter drops these; only compact output prints them.
    #[must_use]
    pub fn empty_stmt(&self) -> Statement {
        Statement::EmptyStatement(self.next_span())
    }

    #[must_use]
    pub fn return_(&self, exprs: Vec<Expression>) -> LastStatement {
        LastStatement::Return(Box::new(ReturnStatement {
            span: self.next_span(),
            exprs: Punctuated::from_items(exprs),
        }))
    }

    #[must_use]
    pub fn break_(&self) -> LastStatement {
        LastStatement::Break(self.next_span())
    }

    #[must_use]
    pub fn continue_(&self) -> LastStatement {
        LastStatement::Continue(self.next_span())
    }

    #[must_use]
    pub fn block(&self, stmts: Vec<Statement>, last: Option<LastStatement>) -> Block {
        Block { span: self.next_span(), stmts, last_stmt: last.map(Box::new) }
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn type_declaration(
        &self,
        is_exported: bool,
        name: &str,
        generics: Option<GenericTypeList>,
        value: Type,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.identifier(name),
            generics: generics.map(Box::new),
            type_value: TypeDeclarationValue::Alias(value),
        }))
    }

    /// # Panics
    ///
    /// Panics if `name` isn't a valid identifier, as [`Synth::identifier`] does.
    #[must_use]
    pub fn type_function(
        &self,
        is_exported: bool,
        name: &str,
        signature: FunctionSignature,
        body: Block,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            is_exported,
            name: self.identifier(name),
            generics: None,
            type_value: TypeDeclarationValue::TypeFunction(Box::new(
                self.function_body(signature, body),
            )),
        }))
    }
}

#[cfg(test)]
mod tests {
    use luck_token::{BinOp, LuaVersion};

    use super::*;

    #[test]
    fn builds_local_with_binary_op() {
        let synth = Synth::new();
        let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("2"));
        let stmt = synth.local(&["x"], vec![sum]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 1);
        let exprs = local.exprs.as_ref().unwrap();
        assert!(matches!(exprs.first().unwrap(), Expression::BinaryOp(_)));
    }

    #[test]
    fn builds_typed_local() {
        let synth = Synth::new();
        let optional = synth.ty_optional(synth.ty_named("number"));
        let stmt = synth
            .local_full(vec![synth.attributed_name("x", Some(optional), None)], vec![synth.nil()]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let first = local.names.first().unwrap();
        let ty = first.type_annotation.as_ref().unwrap();
        assert!(matches!(ty, Type::Optional(_)));
    }

    #[test]
    fn builds_const_declarations() {
        let synth = Synth::new();
        let stmt = synth.const_local(
            vec![synth.attributed_name("frozen", None, None)],
            vec![synth.number_i64(1)],
        );
        let Statement::LocalAssignment(const_local) = &stmt else {
            panic!("expected local assignment");
        };
        assert!(const_local.is_const);

        let plain = synth.local(&["thawed"], vec![synth.number_i64(2)]);
        let Statement::LocalAssignment(plain_local) = &plain else {
            panic!("expected local assignment");
        };
        assert!(!plain_local.is_const);

        let function_stmt = synth.const_function(
            "pinned",
            Vec::new(),
            FunctionSignature::default(),
            synth.block(vec![], None),
        );
        let Statement::LocalFunction(local_function) = &function_stmt else {
            panic!("expected local function");
        };
        assert_eq!(local_function.kind, LocalFunctionKind::Const);
    }

    #[test]
    fn builds_globals() {
        let synth = Synth::new();
        let stmt = synth
            .global_declaration(vec![synth.attributed_name("shared", None, Some("const"))], vec![]);
        let Statement::GlobalDeclaration(declaration) = &stmt else {
            panic!("expected global declaration");
        };
        assert!(declaration.names.first().unwrap().attribute.is_some());

        let function =
            synth.global_function("main", FunctionSignature::default(), synth.block(vec![], None));
        assert!(matches!(function, Statement::GlobalFunction(_)));

        let Statement::GlobalStar(star) = synth.global_star(Some("const")) else {
            panic!("expected global star");
        };
        assert!(star.attribute.is_some());
    }

    #[test]
    fn builds_compound_assignment() {
        let synth = Synth::new();
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
        let synth = Synth::new();
        let stmt = synth.if_(
            synth.boolean(true),
            synth.block(vec![], None),
            vec![(synth.boolean(false), synth.block(vec![], None))],
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
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("pairs"), vec![synth.name_expr("t")]);
        let stmt = synth.generic_for(
            vec![synth.param("k"), synth.param("v")],
            vec![call],
            synth.block(vec![], None),
        );

        let Statement::GenericFor(generic_for) = &stmt else {
            panic!("expected generic for");
        };
        assert_eq!(generic_for.names.len(), 2);
        assert_eq!(generic_for.exprs.len(), 1);
    }

    #[test]
    fn builds_typed_numeric_for() {
        let synth = Synth::new();
        let stmt = synth.numeric_for(
            synth.param_typed("i", synth.ty_named("number")),
            synth.number("1"),
            synth.number("10"),
            None,
            synth.block(vec![], None),
        );
        let Statement::NumericFor(numeric_for) = &stmt else {
            panic!("expected numeric for");
        };
        assert!(numeric_for.type_annotation.is_some());
    }

    #[test]
    fn builds_goto_and_label() {
        let synth = Synth::new();
        assert!(matches!(synth.goto_("done"), Statement::Goto(_)));
        assert!(matches!(synth.label("done"), Statement::Label(_)));
    }

    #[test]
    fn builds_continue_and_breaks() {
        let synth = Synth::new();
        assert!(matches!(synth.continue_(), LastStatement::Continue(_)));
        assert!(matches!(synth.break_(), LastStatement::Break(_)));
        assert!(matches!(synth.break_stmt(), Statement::Break(_)));
        assert!(matches!(synth.empty_stmt(), Statement::EmptyStatement(_)));
    }

    #[test]
    fn call_stmt_spans_stay_unique_for_cloned_calls() {
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![]);
        let first = synth.call_stmt(call.clone());
        let second = synth.call_stmt(call.clone());
        assert_ne!(first.span(), second.span());
        assert_ne!(first.span(), call.span());
    }

    #[test]
    fn builds_function_declaration_with_method() {
        let synth = Synth::new();
        let stmt =
            synth.function_decl(&["a", "b"], Some("greet"), &["self"], synth.block(vec![], None));

        let Statement::FunctionDecl(function_decl) = &stmt else {
            panic!("expected function declaration");
        };
        assert_eq!(function_decl.name.names.len(), 2);
        assert!(function_decl.name.method.is_some());
    }

    #[test]
    fn builds_type_declaration_with_generics() {
        let synth = Synth::new();
        let type_params = synth.generic_type_list(vec![("T", false), ("Rest", true)]);
        let stmt = synth.type_declaration(true, "Alias", Some(type_params), synth.ty_named("T"));

        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(declaration.is_exported);
        assert!(matches!(declaration.type_value, TypeDeclarationValue::Alias(_)));
        let generics = declaration.generics.as_ref().expect("generics present");
        assert_eq!(generics.params.len(), 2);
        assert!(generics.params.last().unwrap().is_pack);
    }

    #[test]
    fn builds_type_function() {
        let synth = Synth::new();
        let signature =
            FunctionSignature { params: vec![synth.param("ty")], ..FunctionSignature::default() };
        let stmt = synth.type_function(false, "Partial", signature, synth.block(vec![], None));
        let Statement::TypeDeclaration(declaration) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(!declaration.is_exported);
        assert!(matches!(declaration.type_value, TypeDeclarationValue::TypeFunction(_)));
    }

    #[test]
    fn builds_exported_locals() {
        let synth = Synth::new().with_version(LuaVersion::Luau);
        let exported = synth.export_local(
            vec![synth.attributed_name("value", Some(synth.ty_named("integer")), None)],
            vec![synth.integer_i64(1)],
        );
        assert!(matches!(
            exported,
            Statement::LocalAssignment(local) if local.is_exported
        ));
    }
}
