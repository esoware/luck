use crate::expr::{
    Expression, Field, FunctionArgs, Suffix, SuffixedExpression, TableConstructor, Var,
};
use crate::function::{FunctionAttribute, FunctionBody};
use crate::stmt::{AttributedName, Block, LastStatement, Statement, TypeDeclarationValue};
use crate::types::{GenericTypeList, Type, TypeField};

/// [`Visitor`](crate::Visitor)'s walk through `&mut`, in the same order and
/// with the same levels, for passes that rewrite the tree in place.
///
/// Override a `visit_*` hook and call `self.walk_*` from it to recurse. Work
/// before the call sees the node on the way down, and work after sees it
/// once its children are rewritten, so one hook can run any number of small
/// rewrites on a node in one walk.
///
/// A rewrite edits the node it's handed. One that restructures it moves the
/// parts it keeps out with `take`, whose placeholder costs no allocation, and
/// builds the new nodes in the tree's arena, which the pass holds. A rewrite
/// that leaves a bare run where it would merge into the one around it splices
/// them with [`SuffixedExpression::splice_prefix`] or
/// [`BinaryOp::splice_operands`](crate::BinaryOp::splice_operands).
pub trait VisitorMut<'ast> {
    /// Called by the walkers on entering each level of nesting, and always
    /// paired with [`VisitorMut::ascend`]. Returning `false` skips what the
    /// level holds. The levels are those [`Visitor::descend`](crate::Visitor::descend)
    /// counts.
    fn descend(&mut self) -> bool {
        true
    }
    fn ascend(&mut self) {}

    fn visit_block(&mut self, block: &mut Block<'ast>) {
        self.walk_block(block);
    }
    fn visit_stmt(&mut self, stmt: &mut Statement<'ast>) {
        self.walk_stmt(stmt);
    }
    fn visit_last_stmt(&mut self, last: &mut LastStatement<'ast>) {
        self.walk_last_stmt(last);
    }
    fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
        self.walk_expr(expr);
    }
    /// Reached from assignments and Luau compound assignments. A name read
    /// as a value is an [`Expression::Name`] instead.
    fn visit_assignment_target(&mut self, target: &mut Var<'ast>) {
        self.walk_assignment_target(target);
    }
    /// Reached from a call statement, an expression, and an assignment
    /// target. Replacing the whole chain is a rewrite of the expression,
    /// statement, or target that holds it.
    fn visit_suffixed(&mut self, chain: &mut SuffixedExpression<'ast>) {
        self.walk_suffixed(chain);
    }
    /// Reached once for each call in a chain, after the part of the chain
    /// before it.
    fn visit_function_args(&mut self, args: &mut FunctionArgs<'ast>) {
        self.walk_function_args(args);
    }
    /// Reached from both a table expression and a `f{...}` argument.
    fn visit_table_constructor(&mut self, table: &mut TableConstructor<'ast>) {
        self.walk_table_constructor(table);
    }
    fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        self.walk_function_body(body);
    }
    fn visit_type(&mut self, ty: &mut Type<'ast>) {
        self.walk_type(ty);
    }
    fn visit_generic_type_list(&mut self, generics: &mut GenericTypeList<'ast>) {
        self.walk_generic_type_list(generics);
    }

    fn walk_block(&mut self, block: &mut Block<'ast>) {
        if self.descend() {
            for stmt in &mut block.stmts {
                self.visit_stmt(stmt);
            }
            if let Some(last) = &mut block.last_stmt {
                self.visit_last_stmt(last);
            }
        }
        self.ascend();
    }

    fn walk_stmt(&mut self, stmt: &mut Statement<'ast>) {
        match stmt {
            Statement::Assignment(assignment) => {
                for target in &mut assignment.targets {
                    self.visit_assignment_target(target);
                }
                for expr in &mut assignment.values {
                    self.visit_expr(expr);
                }
            }
            Statement::FunctionCall(chain) => self.visit_suffixed(chain),
            Statement::DoBlock(do_block) => self.visit_block(&mut do_block.block),
            Statement::WhileLoop(while_loop) => {
                self.visit_expr(&mut while_loop.condition);
                self.visit_block(&mut while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.visit_block(&mut repeat_loop.block);
                self.visit_expr(&mut repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expr(&mut if_stmt.condition);
                self.visit_block(&mut if_stmt.block);
                for clause in &mut if_stmt.elseif_clauses {
                    self.visit_expr(&mut clause.condition);
                    self.visit_block(&mut clause.block);
                }
                if let Some(else_clause) = &mut if_stmt.else_clause {
                    self.visit_block(&mut else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                if let Some(ty) = &mut numeric_for.type_annotation {
                    self.visit_type(ty);
                }
                self.visit_expr(&mut numeric_for.start);
                self.visit_expr(&mut numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    self.visit_expr(step);
                }
                self.visit_block(&mut numeric_for.block);
            }
            Statement::GenericFor(generic_for) => {
                for binding in &mut generic_for.names {
                    if let Some(ty) = &mut binding.type_annotation {
                        self.visit_type(ty);
                    }
                }
                for expr in &mut generic_for.exprs {
                    self.visit_expr(expr);
                }
                self.visit_block(&mut generic_for.block);
            }
            Statement::FunctionDecl(function_decl) => {
                self.walk_function_attributes(&mut function_decl.attributes);
                self.visit_function_body(&mut function_decl.body);
            }
            Statement::LocalFunction(local_function) => {
                self.walk_function_attributes(&mut local_function.attributes);
                self.visit_function_body(&mut local_function.body);
            }
            Statement::LocalAssignment(local_assignment) => {
                self.walk_attributed_names(&mut local_assignment.names);
                for expr in &mut local_assignment.exprs {
                    self.visit_expr(expr);
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_assignment_target(&mut compound.var);
                self.visit_expr(&mut compound.expr);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.visit_function_body(&mut global_function.body);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                self.walk_attributed_names(&mut global_declaration.names);
                for expr in &mut global_declaration.exprs {
                    self.visit_expr(expr);
                }
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                if let Some(generics) = &mut type_declaration.generics {
                    self.visit_generic_type_list(generics);
                }
                match &mut type_declaration.value {
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

    fn walk_last_stmt(&mut self, last: &mut LastStatement<'ast>) {
        match last {
            LastStatement::Return(return_stmt) => {
                for expr in &mut return_stmt.exprs {
                    self.visit_expr(expr);
                }
            }
            LastStatement::Break(_) | LastStatement::Continue(_) | LastStatement::Error(_) => {}
        }
    }

    fn walk_expr(&mut self, expr: &mut Expression<'ast>) {
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
                self.walk_function_attributes(&mut function_def.attributes);
                self.visit_function_body(&mut function_def.body);
            }
            Expression::Suffixed(chain) => self.visit_suffixed(chain),
            Expression::Parenthesized(parenthesized) => {
                if self.descend() {
                    self.visit_expr(&mut parenthesized.expr);
                }
                self.ascend();
            }
            Expression::TableConstructor(table) => self.visit_table_constructor(table),
            Expression::BinaryOp(binary_op) => {
                if self.descend() {
                    self.visit_expr(&mut binary_op.first);
                    for link in &mut binary_op.links {
                        self.visit_expr(&mut link.operand);
                    }
                }
                self.ascend();
            }
            Expression::UnaryOp(unary_op) => {
                if self.descend() {
                    self.visit_expr(&mut unary_op.operand);
                }
                self.ascend();
            }
            // Luau
            Expression::IfExpression(if_expr) => {
                if self.descend() {
                    self.visit_expr(&mut if_expr.condition);
                    self.visit_expr(&mut if_expr.then_expr);
                    for clause in &mut if_expr.elseif_clauses {
                        self.visit_expr(&mut clause.condition);
                        self.visit_expr(&mut clause.expr);
                    }
                    self.visit_expr(&mut if_expr.else_expr);
                }
                self.ascend();
            }
            // Luau
            Expression::InterpolatedString(interpolated) => {
                if self.descend() {
                    for segment in &mut interpolated.segments {
                        self.visit_expr(&mut segment.expr);
                    }
                }
                self.ascend();
            }
            // Luau
            Expression::TypeCast(cast) => {
                if self.descend() {
                    self.visit_expr(&mut cast.expr);
                    self.visit_type(&mut cast.ty);
                }
                self.ascend();
            }
        }
    }

    fn walk_assignment_target(&mut self, target: &mut Var<'ast>) {
        match target {
            Var::Name(_) => {}
            Var::Suffixed(chain) => self.visit_suffixed(chain),
        }
    }

    /// The prefix, then each suffix in order: an index, a call's explicit
    /// type arguments and then its arguments, or a Luau instantiation's type
    /// arguments.
    fn walk_suffixed(&mut self, chain: &mut SuffixedExpression<'ast>) {
        if self.descend() {
            self.visit_expr(&mut chain.prefix);
            for suffix in &mut chain.suffixes {
                match suffix {
                    Suffix::Field(_) => {}
                    Suffix::Index { index, .. } => self.visit_expr(index),
                    Suffix::Call(args) => self.visit_function_args(args),
                    Suffix::MethodCall(call) => {
                        if let Some(type_args) = &mut call.type_args {
                            for ty in &mut type_args.args {
                                self.visit_type(ty);
                            }
                        }
                        self.visit_function_args(&mut call.args);
                    }
                    // Luau
                    Suffix::TypeInstantiation { type_args, .. } => {
                        for ty in &mut type_args.args {
                            self.visit_type(ty);
                        }
                    }
                }
            }
        }
        self.ascend();
    }

    fn walk_function_args(&mut self, args: &mut FunctionArgs<'ast>) {
        match args {
            FunctionArgs::Parenthesized { args: exprs, .. } => {
                for expr in exprs {
                    self.visit_expr(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => self.visit_table_constructor(table),
            FunctionArgs::StringLiteral(_) | FunctionArgs::Error(_) => {}
        }
    }

    fn walk_table_constructor(&mut self, table: &mut TableConstructor<'ast>) {
        if self.descend() {
            for field in &mut table.fields {
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

    fn walk_function_body(&mut self, body: &mut FunctionBody<'ast>) {
        if self.descend() {
            if let Some(generics) = &mut body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in &mut body.params {
                if let Some(ty) = &mut param.type_annotation {
                    self.visit_type(ty);
                }
            }
            if let Some(vararg) = &mut body.vararg
                && let Some(ty) = &mut vararg.type_annotation
            {
                self.visit_type(ty);
            }
            if let Some(return_type) = &mut body.return_type {
                self.visit_type(return_type);
            }
            self.visit_block(&mut body.block);
        }
        self.ascend();
    }

    fn walk_type(&mut self, ty: &mut Type<'ast>) {
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
                    if let Some(generics) = &mut named.generics {
                        for arg in &mut generics.args {
                            self.visit_type(arg);
                        }
                    }
                }
                // typeof embeds a real expression, so semantic passes must see it.
                Type::Typeof(typeof_type) => self.visit_expr(&mut typeof_type.expr),
                Type::Table(table) => {
                    for field in &mut table.fields {
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
                    if let Some(generics) = &mut function_type.generics {
                        self.visit_generic_type_list(generics);
                    }
                    for param in &mut function_type.params {
                        self.visit_type(&mut param.ty);
                    }
                    self.visit_type(&mut function_type.return_type);
                }
                Type::Optional(optional) => self.visit_type(&mut optional.ty),
                Type::Union(union) => {
                    for member in &mut union.types {
                        self.visit_type(member);
                    }
                }
                Type::Intersection(intersection) => {
                    for member in &mut intersection.types {
                        self.visit_type(member);
                    }
                }
                Type::Parenthesized(parenthesized) => self.visit_type(&mut parenthesized.ty),
                Type::Pack(pack) => {
                    for member in &mut pack.types {
                        self.visit_type(member);
                    }
                }
                Type::Variadic(variadic) => self.visit_type(&mut variadic.ty),
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

    fn walk_generic_type_list(&mut self, generics: &mut GenericTypeList<'ast>) {
        for param in &mut generics.params {
            if let Some(default) = &mut param.default {
                self.visit_type(default);
            }
        }
    }

    fn walk_attributed_names(&mut self, names: &mut [AttributedName<'ast>]) {
        for name in names {
            if let Some(ty) = &mut name.type_annotation {
                self.visit_type(ty);
            }
        }
    }

    /// Each attribute's arguments are a level of their own, as the parser
    /// nests them.
    fn walk_function_attributes(&mut self, attributes: &mut [FunctionAttribute<'ast>]) {
        for args in attributes.iter_mut().filter_map(|attribute| attribute.args.as_mut()) {
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
    use crate::synth::Synth;
    use crate::visitor::Visitor;

    struct NilToFalse;

    impl<'ast> VisitorMut<'ast> for NilToFalse {
        fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
            self.walk_expr(expr);
            if let Expression::Nil(span) = *expr {
                *expr = Expression::False(span);
            }
        }
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct NilFalseCounts {
        nils: usize,
        falses: usize,
    }

    impl<'ast> Visitor<'ast> for NilFalseCounts {
        fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
            if matches!(expr, Expression::Nil(_)) {
                self.nils = self.nils.saturating_add(1);
            }
            if matches!(expr, Expression::False(_)) {
                self.falses = self.falses.saturating_add(1);
            }
            self.walk_expr(expr);
        }
    }

    fn rewrite_counts<'ast, Stmts>(synth: Synth<'ast>, stmts: Stmts) -> NilFalseCounts
    where
        Stmts: IntoIterator<Item = Statement<'ast>>,
        Stmts::IntoIter: ExactSizeIterator,
    {
        let mut block = synth.block(stmts, None);
        NilToFalse.visit_block(&mut block);
        let mut counts = NilFalseCounts::default();
        counts.visit_block(&block);
        counts
    }

    const fn all_false(falses: usize) -> NilFalseCounts {
        NilFalseCounts { nils: 0, falses }
    }

    #[test]
    fn rewrites_reach_local_initializers() {
        // local x = nil
        let fixture = Fixture::default();
        let synth = fixture.synth();
        assert_eq!(rewrite_counts(synth, [synth.local(&["x"], [synth.nil()])]), all_false(1));
    }

    #[test]
    fn rewrites_reach_table_fields() {
        // local t = {nil, nil}
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let table = synth.array([synth.nil(), synth.nil()]);
        assert_eq!(rewrite_counts(synth, [synth.local(&["t"], [table])]), all_false(2));
    }

    #[test]
    fn rewrites_reach_function_returns() {
        // local function f() return nil end
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let body = synth.block([], Some(synth.return_([synth.nil()])));
        assert_eq!(rewrite_counts(synth, [synth.local_function("f", &[], body)]), all_false(1));
    }

    #[test]
    fn rewrites_reach_if_conditions_and_branches() {
        // if nil then x = nil else y = nil end
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let then_block = synth.block([synth.assign([synth.name_var("x")], [synth.nil()])], None);
        let else_block = synth.block([synth.assign([synth.name_var("y")], [synth.nil()])], None);
        let if_stmt = synth.if_(synth.nil(), then_block, [], Some(else_block));
        assert_eq!(rewrite_counts(synth, [if_stmt]), all_false(3));
    }

    #[test]
    fn rewrites_reach_loop_conditions_bounds_and_steps() {
        // while nil do end; repeat until nil; for i = nil, nil, nil do end
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let numeric_for = synth.numeric_for(
            synth.param("i"),
            synth.nil(),
            synth.nil(),
            Some(synth.nil()),
            synth.block([], None),
        );
        let stmts = [
            synth.while_(synth.nil(), synth.block([], None)),
            synth.repeat_(synth.block([], None), synth.nil()),
            numeric_for,
        ];
        assert_eq!(rewrite_counts(synth, stmts), all_false(5));
    }

    #[test]
    fn the_default_walk_leaves_the_tree_as_it_was() {
        struct Identity;
        impl VisitorMut<'_> for Identity {}

        // local x = 1 + 2; a.b, c = x, x, built twice from one span counter
        // start, so the two trees are equal before the walk.
        fn build(fixture: &Fixture) -> Block<'_> {
            let synth = fixture.synth();
            let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("2"));
            let targets = [synth.field_var(synth.name_expr("a"), "b"), synth.name_var("c")];
            let values = [synth.name_expr("x"), synth.name_expr("x")];
            synth.block([synth.local(&["x"], [sum]), synth.assign(targets, values)], None)
        }

        let (walked_fixture, untouched_fixture) = (Fixture::default(), Fixture::default());
        let mut walked = build(&walked_fixture);
        Identity.visit_block(&mut walked);
        assert_eq!(walked, build(&untouched_fixture));
    }

    #[test]
    fn a_rewrite_can_restructure_through_take() {
        // return ((x)), with the parentheses removed in place: return x.
        struct Unparenthesize;

        impl<'ast> VisitorMut<'ast> for Unparenthesize {
            fn visit_expr(&mut self, expr: &mut Expression<'ast>) {
                self.walk_expr(expr);
                if let Expression::Parenthesized(parenthesized) = expr {
                    let inner = parenthesized.expr.take();
                    *expr = inner;
                }
            }
        }

        let fixture = Fixture::default();
        let synth = fixture.synth();
        let grouped = synth.parenthesized(synth.parenthesized(synth.name_expr("x")));
        let mut block = synth.block([], Some(synth.return_([grouped])));
        Unparenthesize.visit_block(&mut block);
        let Some(LastStatement::Return(returned)) = &block.last_stmt else {
            panic!("expected a return");
        };
        assert!(matches!(*returned.exprs, [Expression::Name(_)]));
    }

    #[test]
    fn hooks_fire_in_stmt_and_expr_positions() {
        #[derive(Debug, Default, PartialEq, Eq)]
        struct HookCounts {
            chains: usize,
            args: usize,
            tables: usize,
            bodies: usize,
        }

        impl<'ast> VisitorMut<'ast> for HookCounts {
            fn visit_suffixed(&mut self, chain: &mut SuffixedExpression<'ast>) {
                self.chains = self.chains.saturating_add(1);
                self.walk_suffixed(chain);
            }
            fn visit_function_args(&mut self, args: &mut FunctionArgs<'ast>) {
                self.args = self.args.saturating_add(1);
                self.walk_function_args(args);
            }
            fn visit_table_constructor(&mut self, table: &mut TableConstructor<'ast>) {
                self.tables = self.tables.saturating_add(1);
                self.walk_table_constructor(table);
            }
            fn visit_function_body(&mut self, body: &mut FunctionBody<'ast>) {
                self.bodies = self.bodies.saturating_add(1);
                self.walk_function_body(body);
            }
        }

        // f{}(function() end); local t = {}
        let fixture = Fixture::default();
        let synth = fixture.synth();
        let table_call = synth.call_table(synth.name_expr("f"), []);
        let function = synth.function_def(&[], synth.block([], None));
        let mut block = synth.block(
            [
                synth.call_stmt(synth.call(table_call, [function])),
                synth.local(&["t"], [synth.table([])]),
            ],
            None,
        );
        let mut counts = HookCounts::default();
        counts.visit_block(&mut block);
        assert_eq!(counts, HookCounts { chains: 1, args: 2, tables: 2, bodies: 1 });
    }
}
