use crate::chain::Call;
use crate::expr::{
    Expression, Field, FunctionArgs, Suffix, SuffixedExpression, TableConstructor, Var,
};
use crate::function::{FunctionAttribute, FunctionBody};
use crate::stmt::{AttributedName, Block, LastStatement, Statement, TypeDeclarationValue};
use crate::types::{GenericTypeList, Type, TypeField};

/// Each `visit_*` hook defaults to its `walk_*`, which visits the node's
/// children.
///
/// Override the hooks you need and call `self.walk_*` from them to
/// keep recursing. The `walk_*` methods are the default recursion, so
/// overriding one instead of its hook would leave nothing to fall back on.
///
/// Every node comes with the tree's own lifetime, so a visitor can keep the
/// references it's handed. A tree lent for less than its arena's lifetime
/// reads as one of the shorter lifetime, since every node is covariant.
pub trait Visitor<'ast> {
    /// Called by the walkers on entering each level of nesting, and always
    /// paired with [`Visitor::ascend`]. Returning `false` skips what the
    /// level holds.
    ///
    /// A level is a block, a function body, a table constructor, a run of
    /// suffixes, a run of binary operators, a unary operator, a parenthesized
    /// expression, a function attribute's arguments, a Luau if-expression,
    /// interpolated string, or cast, or a Luau type that holds other types or
    /// an expression. These are the levels
    /// [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH) caps. However long a
    /// run is, it's one level.
    fn descend(&mut self) -> bool {
        true
    }
    fn ascend(&mut self) {}

    fn visit_block(&mut self, block: &'ast Block<'ast>) {
        self.walk_block(block);
    }
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        self.walk_stmt(stmt);
    }
    fn visit_last_stmt(&mut self, last: &'ast LastStatement<'ast>) {
        self.walk_last_stmt(last);
    }
    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        self.walk_expr(expr);
    }
    /// Reached from assignments and Luau compound assignments. A name read
    /// as a value is an [`Expression::Name`] instead.
    fn visit_assignment_target(&mut self, target: &'ast Var<'ast>) {
        self.walk_assignment_target(target);
    }
    /// Reached from a call statement, an expression, and an assignment
    /// target.
    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression<'ast>) {
        self.walk_suffixed(chain);
    }
    /// Reached once for each call in a chain, after the part of the chain
    /// before it. The walk of that chain visits the callee, not this hook's.
    fn visit_call(&mut self, call: Call<'ast>) {
        self.walk_call(call);
    }
    /// Reached from both a table expression and a `f{...}` argument.
    fn visit_table_constructor(&mut self, table: &'ast TableConstructor<'ast>) {
        self.walk_table_constructor(table);
    }
    fn visit_function_body(&mut self, body: &'ast FunctionBody<'ast>) {
        self.walk_function_body(body);
    }
    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        self.walk_type(ty);
    }
    fn visit_generic_type_list(&mut self, generics: &'ast GenericTypeList<'ast>) {
        self.walk_generic_type_list(generics);
    }

    fn walk_block(&mut self, block: &'ast Block<'ast>) {
        if self.descend() {
            for stmt in &block.stmts {
                self.visit_stmt(stmt);
            }
            if let Some(last) = &block.last_stmt {
                self.visit_last_stmt(last);
            }
        }
        self.ascend();
    }

    fn walk_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        match stmt {
            Statement::Assignment(assignment) => {
                for target in &assignment.targets {
                    self.visit_assignment_target(target);
                }
                for expr in &assignment.values {
                    self.visit_expr(expr);
                }
            }
            Statement::FunctionCall(chain) => self.visit_suffixed(chain),
            Statement::DoBlock(do_block) => self.visit_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&while_loop.condition);
                self.visit_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.visit_block(&repeat_loop.block);
                self.visit_expr(&repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expr(&if_stmt.condition);
                self.visit_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.visit_expr(&clause.condition);
                    self.visit_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.visit_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                if let Some(ty) = &numeric_for.type_annotation {
                    self.visit_type(ty);
                }
                self.visit_expr(&numeric_for.start);
                self.visit_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expr(step);
                }
                self.visit_block(&numeric_for.block);
            }
            Statement::GenericFor(generic_for) => {
                for binding in &generic_for.names {
                    if let Some(ty) = &binding.type_annotation {
                        self.visit_type(ty);
                    }
                }
                for expr in &generic_for.exprs {
                    self.visit_expr(expr);
                }
                self.visit_block(&generic_for.block);
            }
            Statement::FunctionDecl(function_decl) => {
                self.walk_function_attributes(&function_decl.attributes);
                self.visit_function_body(&function_decl.body);
            }
            Statement::LocalFunction(local_function) => {
                self.walk_function_attributes(&local_function.attributes);
                self.visit_function_body(&local_function.body);
            }
            Statement::LocalAssignment(local_assignment) => {
                self.walk_attributed_names(&local_assignment.names);
                for expr in &local_assignment.exprs {
                    self.visit_expr(expr);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_assignment_target(&compound.var);
                self.visit_expr(&compound.expr);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                self.walk_attributed_names(&global_declaration.names);
                for expr in &global_declaration.exprs {
                    self.visit_expr(expr);
                }
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                if let Some(generics) = &type_declaration.generics {
                    self.visit_generic_type_list(generics);
                }
                match &type_declaration.value {
                    TypeDeclarationValue::Alias(ty) => self.visit_type(ty),
                    TypeDeclarationValue::TypeFunction(body) => self.visit_function_body(body),
                }
            }
            Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
    }

    fn walk_last_stmt(&mut self, last: &'ast LastStatement<'ast>) {
        match last {
            LastStatement::Return(return_stmt) => {
                for expr in &return_stmt.exprs {
                    self.visit_expr(expr);
                }
            }
            LastStatement::Break(_) | LastStatement::Continue(_) | LastStatement::Error(_) => {}
        }
    }

    fn walk_expr(&mut self, expr: &'ast Expression<'ast>) {
        match expr {
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Name(_)
            | Expression::Error(_) => {}
            Expression::FunctionDef(function_def) => {
                self.walk_function_attributes(&function_def.attributes);
                self.visit_function_body(&function_def.body);
            }
            Expression::Suffixed(chain) => self.visit_suffixed(chain),
            Expression::Parenthesized(parenthesized) => {
                if self.descend() {
                    self.visit_expr(&parenthesized.expr);
                }
                self.ascend();
            }
            Expression::TableConstructor(table) => self.visit_table_constructor(table),
            Expression::BinaryOp(binary_op) => {
                if self.descend() {
                    self.visit_expr(&binary_op.first);
                    for link in &binary_op.links {
                        self.visit_expr(&link.operand);
                    }
                }
                self.ascend();
            }
            Expression::UnaryOp(unary_op) => {
                if self.descend() {
                    self.visit_expr(&unary_op.operand);
                }
                self.ascend();
            }
            // Luau
            Expression::IfExpression(if_expr) => {
                if self.descend() {
                    self.visit_expr(&if_expr.condition);
                    self.visit_expr(&if_expr.then_expr);
                    for clause in &if_expr.elseif_clauses {
                        self.visit_expr(&clause.condition);
                        self.visit_expr(&clause.expr);
                    }
                    self.visit_expr(&if_expr.else_expr);
                }
                self.ascend();
            }
            // Luau
            Expression::InterpolatedString(interpolated) => {
                if self.descend() {
                    for segment in &interpolated.segments {
                        self.visit_expr(&segment.expr);
                    }
                }
                self.ascend();
            }
            // Luau
            Expression::TypeCast(cast) => {
                if self.descend() {
                    self.visit_expr(&cast.expr);
                    self.visit_type(&cast.ty);
                }
                self.ascend();
            }
        }
    }

    fn walk_assignment_target(&mut self, target: &'ast Var<'ast>) {
        match target {
            Var::Name(_) => {}
            Var::Suffixed(chain) => self.visit_suffixed(chain),
        }
    }

    fn walk_suffixed(&mut self, chain: &'ast SuffixedExpression<'ast>) {
        if self.descend() {
            self.visit_expr(&chain.prefix);
            for (i, suffix) in chain.suffixes.iter().enumerate() {
                match suffix {
                    Suffix::Field(_) => {}
                    Suffix::Index { index, .. } => self.visit_expr(index),
                    // Built in one place, so each level of nested calls holds
                    // one `Call` in its frame, not one per kind.
                    Suffix::Call(_) | Suffix::MethodCall(_) => {
                        if let Some(call) = Call::new(chain.head(i), suffix) {
                            self.visit_call(call);
                        }
                    }
                    // Luau
                    Suffix::TypeInstantiation { type_args, .. } => {
                        for ty in &type_args.args {
                            self.visit_type(ty);
                        }
                    }
                }
            }
        }
        self.ascend();
    }

    /// Explicit type arguments, then the arguments.
    fn walk_call(&mut self, call: Call<'ast>) {
        if let Some(type_args) = call.type_args {
            for ty in &type_args.args {
                self.visit_type(ty);
            }
        }
        match call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                for expr in args {
                    self.visit_expr(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => self.visit_table_constructor(table),
            FunctionArgs::StringLiteral(_) | FunctionArgs::Error(_) => {}
        }
    }

    fn walk_table_constructor(&mut self, table: &'ast TableConstructor<'ast>) {
        if self.descend() {
            for field in &table.fields {
                match field {
                    Field::Bracketed { key, value, .. } => {
                        self.visit_expr(key);
                        self.visit_expr(value);
                    }
                    Field::Named { value, .. } | Field::Positional(value) => {
                        self.visit_expr(value);
                    }
                }
            }
        }
        self.ascend();
    }

    fn walk_function_body(&mut self, body: &'ast FunctionBody<'ast>) {
        if self.descend() {
            if let Some(generics) = &body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in &body.params {
                if let Some(ty) = &param.type_annotation {
                    self.visit_type(ty);
                }
            }
            if let Some(vararg) = &body.vararg
                && let Some(ty) = &vararg.type_annotation
            {
                self.visit_type(ty);
            }
            if let Some(return_type) = &body.return_type {
                self.visit_type(return_type);
            }
            self.visit_block(&body.block);
        }
        self.ascend();
    }

    fn walk_type(&mut self, ty: &'ast Type<'ast>) {
        if matches!(
            ty,
            Type::Name(_)
                | Type::Nil(_)
                | Type::True(_)
                | Type::False(_)
                | Type::StringLiteral(_)
                | Type::Number(_)
                | Type::GenericPack(_)
                | Type::Error(_)
        ) {
            return;
        }
        if self.descend() {
            match ty {
                Type::Named(named) => {
                    if let Some(generics) = &named.generics {
                        for arg in &generics.args {
                            self.visit_type(arg);
                        }
                    }
                }
                // typeof embeds a real expression, so semantic passes must see it.
                Type::Typeof(typeof_type) => self.visit_expr(&typeof_type.expr),
                Type::Table(table) => {
                    for field in &table.fields {
                        match field {
                            TypeField::Named { value, .. } | TypeField::Array { value, .. } => {
                                self.visit_type(value);
                            }
                            TypeField::Indexer { key, value, .. } => {
                                self.visit_type(key);
                                self.visit_type(value);
                            }
                        }
                    }
                }
                Type::Function(function_type) => {
                    if let Some(generics) = &function_type.generics {
                        self.visit_generic_type_list(generics);
                    }
                    for param in &function_type.params {
                        self.visit_type(&param.ty);
                    }
                    self.visit_type(&function_type.return_type);
                }
                Type::Optional(optional) => self.visit_type(&optional.ty),
                Type::Union(union) => {
                    for member in &union.types {
                        self.visit_type(member);
                    }
                }
                Type::Intersection(intersection) => {
                    for member in &intersection.types {
                        self.visit_type(member);
                    }
                }
                Type::Parenthesized(parenthesized) => self.visit_type(&parenthesized.ty),
                Type::Pack(pack) => {
                    for member in &pack.types {
                        self.visit_type(member);
                    }
                }
                Type::Variadic(variadic) => self.visit_type(&variadic.ty),
                Type::Name(_)
                | Type::Nil(_)
                | Type::True(_)
                | Type::False(_)
                | Type::StringLiteral(_)
                | Type::Number(_)
                | Type::GenericPack(_)
                | Type::Error(_) => {}
            }
        }
        self.ascend();
    }

    fn walk_generic_type_list(&mut self, generics: &'ast GenericTypeList<'ast>) {
        for param in &generics.params {
            if let Some(default) = &param.default {
                self.visit_type(default);
            }
        }
    }

    fn walk_attributed_names(&mut self, names: &'ast [AttributedName<'ast>]) {
        for name in names {
            if let Some(ty) = &name.type_annotation {
                self.visit_type(ty);
            }
        }
    }

    /// Each attribute's arguments are a level of their own, as the parser
    /// nests them.
    fn walk_function_attributes(&mut self, attributes: &'ast [FunctionAttribute<'ast>]) {
        for args in attributes.iter().filter_map(|attribute| attribute.args.as_ref()) {
            if self.descend() {
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            self.ascend();
        }
    }
}

#[cfg(test)]
mod tests {
    use luck_token::BinOp;

    use super::*;
    use crate::fixtures::Fixture;
    use crate::synth::{FunctionSignature, InterpolatedStringPart, Synth};

    #[derive(Debug, Default, PartialEq, Eq)]
    struct HookCounts {
        statements: usize,
        expressions: usize,
        calls: usize,
        tables: usize,
        bodies: usize,
        generic_lists: usize,
    }

    impl<'ast> Visitor<'ast> for HookCounts {
        fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
            self.statements = self.statements.saturating_add(1);
            self.walk_stmt(stmt);
        }
        fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
            self.expressions = self.expressions.saturating_add(1);
            self.walk_expr(expr);
        }
        fn visit_call(&mut self, call: Call<'ast>) {
            self.calls = self.calls.saturating_add(1);
            self.walk_call(call);
        }
        fn visit_table_constructor(&mut self, table: &'ast TableConstructor<'ast>) {
            self.tables = self.tables.saturating_add(1);
            self.walk_table_constructor(table);
        }
        fn visit_function_body(&mut self, body: &'ast FunctionBody<'ast>) {
            self.bodies = self.bodies.saturating_add(1);
            self.walk_function_body(body);
        }
        fn visit_generic_type_list(&mut self, generics: &'ast GenericTypeList<'ast>) {
            self.generic_lists = self.generic_lists.saturating_add(1);
            self.walk_generic_type_list(generics);
        }
    }

    fn hook_counts(block: &Block<'_>) -> HookCounts {
        let mut counts = HookCounts::default();
        counts.visit_block(block);
        counts
    }

    fn expr_count<'ast, Stmts>(synth: Synth<'ast>, stmts: Stmts) -> usize
    where
        Stmts: IntoIterator<Item = Statement<'ast>>,
        Stmts::IntoIter: ExactSizeIterator,
    {
        hook_counts(&synth.block(stmts, None)).expressions
    }

    #[test]
    fn visitor_reaches_binary_operands() {
        // local x = 1 + 1: the sum and both operands.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("1"));
        assert_eq!(expr_count(synth, [synth.local(&["x"], [sum])]), 3);
    }

    #[test]
    fn visitor_reaches_table_fields() {
        // local t = {1, 1, 1}: the constructor and its three values.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let table = synth.array([synth.number("1"), synth.number("1"), synth.number("1")]);
        assert_eq!(expr_count(synth, [synth.local(&["t"], [table])]), 4);
    }

    #[test]
    fn visit_stmt_skips_the_last_stmt() {
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let block = synth.block(
            [synth.local(&["x"], [synth.number("1")]), synth.local(&["y"], [synth.number("1")])],
            Some(synth.return_([synth.name_expr("x")])),
        );
        assert_eq!(hook_counts(&block).statements, 2);
    }

    #[test]
    fn visitor_reaches_function_bodies() {
        // local function f(a) return a + 1 end: the sum and both operands.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.number("1"));
        let body = synth.block([], Some(synth.return_([sum])));
        assert_eq!(expr_count(synth, [synth.local_function("f", &["a"], body)]), 3);
    }

    #[test]
    fn visitor_reaches_if_conditions_and_branches() {
        // if x then y = 1 else y = nil end: x, 1, and nil.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let then_block =
            synth.block([synth.assign([synth.name_var("y")], [synth.number("1")])], None);
        let else_block = synth.block([synth.assign([synth.name_var("y")], [synth.nil()])], None);
        let if_stmt = synth.if_(synth.name_expr("x"), then_block, [], Some(else_block));
        assert_eq!(expr_count(synth, [if_stmt]), 3);
    }

    #[test]
    fn visitor_reaches_numeric_for_bounds_and_step() {
        // for i = 1, 10, 2 do end
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let numeric_for = synth.numeric_for(
            synth.param("i"),
            synth.number("1"),
            synth.number("10"),
            Some(synth.number("2")),
            synth.block([], None),
        );
        assert_eq!(expr_count(synth, [numeric_for]), 3);
    }

    #[test]
    fn visitor_reaches_interpolated_expressions_and_attribute_arguments() {
        // local f = @[deprecated("x")] function() return `{a}{b}` end: the
        // function, its argument, the string, and both holes.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let interpolated = synth.interpolated_string([
            InterpolatedStringPart::Expr(synth.name_expr("a")),
            InterpolatedStringPart::Expr(synth.name_expr("b")),
        ]);
        let function = synth.function_def_full(
            [synth.function_attribute_with_args("deprecated", [synth.string("x")])],
            FunctionSignature::default(),
            synth.block([], Some(synth.return_([interpolated]))),
        );
        assert_eq!(expr_count(synth, [synth.local(&["f"], [function])]), 5);
    }

    #[test]
    fn call_stmt_visits_callee_and_args_but_no_call_expr() {
        // f(x): the call is a statement here, so only `f` and `x` count.
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let call = synth.call(synth.name_expr("f"), [synth.name_expr("x")]);
        assert_eq!(expr_count(synth, [synth.call_stmt(call)]), 2);
    }

    #[test]
    fn hooks_fire_in_stmt_and_expr_positions() {
        // f{}(function<T>() end); local t = {}
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let generic_function = synth.function_def_full(
            [],
            FunctionSignature {
                generics: Some(synth.generic_type_list([("T", false)])),
                ..FunctionSignature::default()
            },
            synth.block([], None),
        );
        let table_call = synth.call_table(synth.name_expr("f"), []);
        let outer_call = synth.call(table_call, [generic_function]);
        let block = synth
            .block([synth.call_stmt(outer_call), synth.local(&["t"], [synth.table([])])], None);
        let counts = hook_counts(&block);
        assert_eq!(counts.calls, 2, "{counts:?}");
        assert_eq!(counts.tables, 2, "{counts:?}");
        assert_eq!(counts.bodies, 1, "{counts:?}");
        assert_eq!(counts.generic_lists, 1, "{counts:?}");
    }

    #[test]
    fn a_visitor_keeps_the_nodes_it_is_handed() {
        struct NameCollector<'ast>(Vec<&'ast str>);

        impl<'ast> Visitor<'ast> for NameCollector<'ast> {
            fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
                if let Expression::Name(name) = expr {
                    self.0.push(name.text);
                }
                self.walk_expr(expr);
            }
        }

        let fixture = Fixture::default();
        let synth = fixture.synth();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.name_expr("b"));
        let block = synth.block([], Some(synth.return_([sum])));
        let mut collector = NameCollector(Vec::new());
        collector.visit_block(&block);
        assert_eq!(collector.0, ["a", "b"]);
    }

    #[test]
    fn a_tree_is_send_and_sync() {
        fn assert_send_sync<T>()
        where
            T: Send + Sync,
        {
        }
        assert_send_sync::<Block<'_>>();
        assert_send_sync::<Expression<'_>>();
        assert_send_sync::<Type<'_>>();
    }
}
