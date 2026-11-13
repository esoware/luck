use crate::depth::regrouped_levels;
use crate::expr::{Expression, Field, FunctionArgs, FunctionCall, TableConstructor, Var};
use crate::function::FunctionBody;
use crate::punctuated::Punctuated;
use crate::stmt::{AttributedName, Block, LastStatement, Statement, TypeDeclarationValue};
use crate::types::{GenericTypeList, Type, TypeField};

/// Each `visit_*` hook defaults to its `walk_*`, which visits the node's
/// children.
///
/// Override the hooks you need and call `self.walk_*` from them to
/// keep recursing. The `walk_*` methods are the default recursion, so
/// overriding one instead of its hook would leave nothing to fall back on.
pub trait Visitor<'ast> {
    /// Called by the walkers on entering each level of nesting, and always
    /// paired with [`Visitor::ascend`]. Returning `false` skips what the
    /// level holds.
    ///
    /// A level is a block, a function body, a table constructor, a call, an
    /// index or field access, an operator, a parenthesized expression, a Luau
    /// if-expression, interpolated string, cast, or instantiation, or a Luau
    /// type that holds other types or an expression. These are the levels
    /// [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH) caps.
    fn descend(&mut self) -> bool {
        true
    }
    fn ascend(&mut self) {}

    fn visit_block(&mut self, block: &'ast Block) {
        self.walk_block(block);
    }
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        self.walk_stmt(stmt);
    }
    fn visit_last_stmt(&mut self, last: &'ast LastStatement) {
        self.walk_last_stmt(last);
    }
    fn visit_expr(&mut self, expr: &'ast Expression) {
        self.walk_expr(expr);
    }
    fn visit_var(&mut self, var: &'ast Var) {
        self.walk_var(var);
    }
    /// Reached from both a call statement and a call expression.
    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.walk_function_call(call);
    }
    /// Reached from both a table expression and a `f{...}` argument.
    fn visit_table_constructor(&mut self, table: &'ast TableConstructor) {
        self.walk_table_constructor(table);
    }
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.walk_function_body(body);
    }
    fn visit_type(&mut self, type_value: &'ast Type) {
        self.walk_type(type_value);
    }
    fn visit_generic_type_list(&mut self, generics: &'ast GenericTypeList) {
        self.walk_generic_type_list(generics);
    }

    fn walk_block(&mut self, block: &'ast Block) {
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

    fn walk_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    self.visit_var(var);
                }
                for expr in assignment.values.iter() {
                    self.visit_expr(expr);
                }
            }
            Statement::FunctionCall(call_stmt) => self.visit_function_call(call_stmt),
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
                if let Some(loop_variable_type) = &numeric_for.type_annotation {
                    self.visit_type(loop_variable_type);
                }
                self.visit_expr(&numeric_for.start);
                self.visit_expr(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expr(step);
                }
                self.visit_block(&numeric_for.block);
            }
            Statement::GenericFor(generic_for) => {
                for binding in generic_for.names.iter() {
                    if let Some(binding_type) = &binding.type_annotation {
                        self.visit_type(binding_type);
                    }
                }
                for expr in generic_for.exprs.iter() {
                    self.visit_expr(expr);
                }
                self.visit_block(&generic_for.block);
            }
            Statement::FunctionDecl(function_decl) => {
                self.visit_function_body(&function_decl.body);
            }
            Statement::LocalFunction(local_function) => {
                self.visit_function_body(&local_function.body);
            }
            Statement::LocalAssignment(local_assignment) => {
                self.walk_attributed_names(&local_assignment.names);
                if let Some(exprs) = &local_assignment.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_var(&compound.var);
                self.visit_expr(&compound.expr);
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                self.visit_function_body(&global_function.body);
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                self.walk_attributed_names(&global_declaration.names);
                if let Some(exprs) = &global_declaration.exprs {
                    for expr in exprs.iter() {
                        self.visit_expr(expr);
                    }
                }
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                if let Some(generics) = &type_declaration.generics {
                    self.visit_generic_type_list(generics);
                }
                match &type_declaration.type_value {
                    TypeDeclarationValue::Alias(alias_type) => self.visit_type(alias_type),
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

    fn walk_last_stmt(&mut self, last: &'ast LastStatement) {
        match last {
            LastStatement::Return(return_stmt) => {
                for expr in return_stmt.exprs.iter() {
                    self.visit_expr(expr);
                }
            }
            LastStatement::Break(_) | LastStatement::Continue(_) | LastStatement::Error(_) => {}
        }
    }

    fn walk_expr(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
            Expression::FunctionDef(function_def) => {
                self.visit_function_body(&function_def.body);
            }
            Expression::Var(var) => {
                self.visit_var(var);
            }
            Expression::FunctionCall(call) => {
                self.visit_function_call(call);
            }
            Expression::Parenthesized(parenthesized) => {
                if self.descend() {
                    self.visit_expr(&parenthesized.expr);
                }
                self.ascend();
            }
            Expression::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            Expression::BinaryOp(binary_op) => {
                if self.descend() {
                    let (lhs_levels, rhs_levels) = regrouped_levels(binary_op);
                    if lhs_levels == 0 && rhs_levels == 0 {
                        self.visit_expr(&binary_op.lhs);
                        self.visit_expr(&binary_op.rhs);
                    } else {
                        visit_regrouped(self, &binary_op.lhs, lhs_levels);
                        visit_regrouped(self, &binary_op.rhs, rhs_levels);
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
            Expression::InterpolatedString(interpolated_string) => {
                if self.descend() {
                    for segment in &interpolated_string.segments {
                        if let Some(interpolated) = &segment.expr {
                            self.visit_expr(interpolated);
                        }
                    }
                }
                self.ascend();
            }
            // Luau
            Expression::TypeCast(cast) => {
                if self.descend() {
                    self.visit_expr(&cast.expr);
                    self.visit_type(&cast.type_annotation);
                }
                self.ascend();
            }
            // Luau
            Expression::TypeInstantiation(instantiation) => {
                if self.descend() {
                    self.visit_expr(&instantiation.expr);
                    for arg in instantiation.type_args.args.iter() {
                        self.visit_type(arg);
                    }
                }
                self.ascend();
            }
        }
    }

    fn walk_var(&mut self, var: &'ast Var) {
        match var {
            Var::Name(_) => {}
            Var::Index(index_expr) => {
                if self.descend() {
                    self.visit_expr(&index_expr.prefix);
                    self.visit_expr(&index_expr.index);
                }
                self.ascend();
            }
            Var::FieldAccess(field_access) => {
                if self.descend() {
                    self.visit_expr(&field_access.prefix);
                }
                self.ascend();
            }
        }
    }

    fn walk_function_call(&mut self, call: &'ast FunctionCall) {
        if self.descend() {
            self.visit_expr(&call.callee);
            if let Some(type_args) = &call.explicit_type_args {
                for type_arg in type_args.args.iter() {
                    self.visit_type(type_arg);
                }
            }
            self.walk_function_args(&call.args);
        }
        self.ascend();
    }

    fn walk_function_args(&mut self, args: &'ast FunctionArgs) {
        match args {
            FunctionArgs::Parenthesized { args: exprs, .. } => {
                for expr in exprs.iter() {
                    self.visit_expr(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn walk_table_constructor(&mut self, table: &'ast TableConstructor) {
        if self.descend() {
            for field in table.fields.iter() {
                match field {
                    Field::Bracketed { key, value, .. } => {
                        self.visit_expr(key);
                        self.visit_expr(value);
                    }
                    Field::Named { value, .. } | Field::Positional { value, .. } => {
                        self.visit_expr(value);
                    }
                }
            }
        }
        self.ascend();
    }

    fn walk_function_body(&mut self, body: &'ast FunctionBody) {
        if self.descend() {
            if let Some(generics) = &body.generics {
                self.visit_generic_type_list(generics);
            }
            for param in body.params.iter() {
                if let Some(param_type) = &param.type_annotation {
                    self.visit_type(param_type);
                }
            }
            if let Some(vararg) = &body.vararg
                && let Some(vararg_type) = &vararg.type_annotation
            {
                self.visit_type(vararg_type);
            }
            if let Some(return_type) = &body.return_type {
                self.visit_type(return_type);
            }
            self.visit_block(&body.block);
        }
        self.ascend();
    }

    fn walk_type(&mut self, type_value: &'ast Type) {
        if matches!(
            type_value,
            Type::Name { .. } | Type::Singleton(_) | Type::GenericPack(_) | Type::Error(_)
        ) {
            return;
        }
        if self.descend() {
            match type_value {
                Type::Named(named) => {
                    if let Some(generics) = &named.generics {
                        for arg in generics.args.iter() {
                            self.visit_type(arg);
                        }
                    }
                }
                // typeof embeds a real expression, so semantic passes must see it.
                Type::Typeof(typeof_type) => {
                    self.visit_expr(&typeof_type.expr);
                }
                Type::Table(table) => {
                    for field in table.fields.iter() {
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
                    for param in function_type.params.iter() {
                        self.visit_type(&param.type_value);
                    }
                    self.visit_type(&function_type.return_type);
                }
                Type::Optional(optional) => {
                    self.visit_type(&optional.type_value);
                }
                Type::Union(union) => {
                    for item in union.types.iter() {
                        self.visit_type(item);
                    }
                }
                Type::Intersection(intersection) => {
                    for item in intersection.types.iter() {
                        self.visit_type(item);
                    }
                }
                Type::Parenthesized(parenthesized) => self.visit_type(&parenthesized.type_value),
                Type::Pack(pack) => {
                    for item in pack.types.iter() {
                        self.visit_type(item);
                    }
                }
                Type::Variadic(variadic) => self.visit_type(&variadic.type_value),
                Type::Name { .. } | Type::Singleton(_) | Type::GenericPack(_) | Type::Error(_) => {}
            }
        }
        self.ascend();
    }

    fn walk_generic_type_list(&mut self, generics: &'ast GenericTypeList) {
        for param in generics.params.iter() {
            if let Some(default) = &param.default {
                self.visit_type(default);
            }
        }
    }

    fn walk_attributed_names(&mut self, names: &'ast Punctuated<AttributedName>) {
        for name in names.iter() {
            if let Some(name_type) = &name.type_annotation {
                self.visit_type(name_type);
            }
        }
    }
}

/// Visits an operand `levels` deeper than it sits in the tree, where it
/// ends up once its operator is printed and read back.
fn visit_regrouped<'ast, V>(visitor: &mut V, operand: &'ast Expression, levels: usize)
where
    V: Visitor<'ast> + ?Sized,
{
    let mut entered = 0usize;
    let mut is_open = true;
    while is_open && entered < levels {
        is_open = visitor.descend();
        entered = entered.saturating_add(1);
    }
    if is_open {
        visitor.visit_expr(operand);
    }
    for _ in 0..entered {
        visitor.ascend();
    }
}

#[cfg(test)]
mod tests {
    use luck_token::BinOp;

    use super::*;
    use crate::synth::{FunctionSignature, Synth};

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
        fn visit_stmt(&mut self, stmt: &'ast Statement) {
            self.statements = self.statements.saturating_add(1);
            self.walk_stmt(stmt);
        }
        fn visit_expr(&mut self, expr: &'ast Expression) {
            self.expressions = self.expressions.saturating_add(1);
            self.walk_expr(expr);
        }
        fn visit_function_call(&mut self, call: &'ast FunctionCall) {
            self.calls = self.calls.saturating_add(1);
            self.walk_function_call(call);
        }
        fn visit_table_constructor(&mut self, table: &'ast TableConstructor) {
            self.tables = self.tables.saturating_add(1);
            self.walk_table_constructor(table);
        }
        fn visit_function_body(&mut self, body: &'ast FunctionBody) {
            self.bodies = self.bodies.saturating_add(1);
            self.walk_function_body(body);
        }
        fn visit_generic_type_list(&mut self, generics: &'ast GenericTypeList) {
            self.generic_lists = self.generic_lists.saturating_add(1);
            self.walk_generic_type_list(generics);
        }
    }

    fn hook_counts(block: &Block) -> HookCounts {
        let mut counts = HookCounts::default();
        counts.visit_block(block);
        counts
    }

    fn expr_count(stmts: Vec<Statement>) -> usize {
        let synth = Synth::new();
        hook_counts(&synth.block(stmts, None)).expressions
    }

    #[test]
    fn visitor_reaches_binary_operands() {
        // local x = 1 + 1: the sum and both operands.
        let synth = Synth::new();
        let sum = synth.binary_op(synth.number("1"), BinOp::Add, synth.number("1"));
        assert_eq!(expr_count(vec![synth.local(&["x"], vec![sum])]), 3);
    }

    #[test]
    fn visitor_reaches_table_fields() {
        // local t = {1, 1, 1}: the constructor and its three values.
        let synth = Synth::new();
        let table = synth.array(vec![synth.number("1"), synth.number("1"), synth.number("1")]);
        assert_eq!(expr_count(vec![synth.local(&["t"], vec![table])]), 4);
    }

    #[test]
    fn visit_stmt_skips_the_last_stmt() {
        let synth = Synth::new();
        let block = synth.block(
            vec![
                synth.local(&["x"], vec![synth.number("1")]),
                synth.local(&["y"], vec![synth.number("1")]),
            ],
            Some(synth.return_(vec![synth.name_expr("x")])),
        );
        assert_eq!(hook_counts(&block).statements, 2);
    }

    #[test]
    fn visitor_reaches_function_bodies() {
        // local function f(a) return a + 1 end: the sum and both operands.
        let synth = Synth::new();
        let sum = synth.binary_op(synth.name_expr("a"), BinOp::Add, synth.number("1"));
        let body = synth.block(vec![], Some(synth.return_(vec![sum])));
        assert_eq!(expr_count(vec![synth.local_function("f", &["a"], body)]), 3);
    }

    #[test]
    fn visitor_reaches_if_conditions_and_branches() {
        // if x then y = 1 else y = nil end: x, 1, and nil.
        let synth = Synth::new();
        let then_block = synth
            .block(vec![synth.assign(vec![synth.name_var("y")], vec![synth.number("1")])], None);
        let else_block =
            synth.block(vec![synth.assign(vec![synth.name_var("y")], vec![synth.nil()])], None);
        let if_stmt = synth.if_(synth.name_expr("x"), then_block, vec![], Some(else_block));
        assert_eq!(expr_count(vec![if_stmt]), 3);
    }

    #[test]
    fn visitor_reaches_numeric_for_bounds_and_step() {
        // for i = 1, 10, 2 do end
        let synth = Synth::new();
        let numeric_for = synth.numeric_for(
            synth.param("i"),
            synth.number("1"),
            synth.number("10"),
            Some(synth.number("2")),
            synth.block(vec![], None),
        );
        assert_eq!(expr_count(vec![numeric_for]), 3);
    }

    #[test]
    fn call_stmt_visits_callee_and_args_but_no_call_expr() {
        // f(x): the call is a statement here, so only `f` and `x` count.
        let synth = Synth::new();
        let call = synth.call(synth.name_expr("f"), vec![synth.name_expr("x")]);
        assert_eq!(expr_count(vec![synth.call_stmt(call)]), 2);
    }

    #[test]
    fn hooks_fire_in_stmt_and_expr_positions() {
        // f{}(function<T>() end); local t = {}
        let synth = Synth::new();
        let generic_function = synth.function_def_full(
            Vec::new(),
            FunctionSignature {
                generics: Some(synth.generic_type_list(vec![("T", false)])),
                ..FunctionSignature::default()
            },
            synth.block(vec![], None),
        );
        let table_call = synth.call_table(synth.name_expr("f"), vec![]);
        let outer_call = synth.call(table_call, vec![generic_function]);
        let block = synth.block(
            vec![synth.call_stmt(outer_call), synth.local(&["t"], vec![synth.table(vec![])])],
            None,
        );
        let counts = hook_counts(&block);
        assert_eq!(counts.calls, 2, "{counts:?}");
        assert_eq!(counts.tables, 2, "{counts:?}");
        assert_eq!(counts.bodies, 1, "{counts:?}");
        assert_eq!(counts.generic_lists, 1, "{counts:?}");
    }
}
