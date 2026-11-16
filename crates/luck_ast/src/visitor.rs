use crate::chain::Call;
use crate::depth::{
    cast_operand_needs_parens, first_operand_needs_parens, intersection_member_needs_parens,
    link_operand_needs_parens, optional_inner_needs_parens, unary_operand_needs_parens,
    union_member_needs_parens,
};
use crate::expr::{
    Expression, Field, FunctionArgs, Suffix, SuffixedExpression, TableConstructor, Var,
};
use crate::function::FunctionBody;
use crate::punctuated::Punctuated;
use crate::query::is_prefix_expr;
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
    /// A level is what the reference compilers count: each statement, each
    /// expression they read with `subexpr` or `parseExpr`, and each Luau
    /// type. A run's first operand, a suffix chain's prefix, a cast's
    /// operand, a union's or intersection's members, and the type under a
    /// run of `?`s share their parent's level, unless the tree holds a shape
    /// the parser reads only inside parentheses, which open one. These are
    /// the levels [`MAX_NESTING_DEPTH`](crate::MAX_NESTING_DEPTH) caps.
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
    /// Reached from assignments and Luau compound assignments. A name read
    /// as a value is an [`Expression::Name`] instead.
    fn visit_assignment_target(&mut self, target: &'ast Var) {
        self.walk_assignment_target(target);
    }
    /// Reached from a call statement, an expression, and an assignment
    /// target.
    fn visit_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        self.walk_suffixed(chain);
    }
    /// Reached once for each call in a chain, after the part of the chain
    /// before it. The walk of that chain visits the callee, not this hook's.
    fn visit_call(&mut self, call: Call<'ast>) {
        self.walk_call(call);
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
        for stmt in &block.stmts {
            if self.descend() {
                self.visit_stmt(stmt);
            }
            self.ascend();
        }
        if let Some(last) = &block.last_stmt {
            if self.descend() {
                self.visit_last_stmt(last);
            }
            self.ascend();
        }
    }

    fn walk_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for target in assignment.targets.iter() {
                    self.visit_assignment_target(target);
                }
                for expr in assignment.values.iter() {
                    visit_nested_expr(self, expr);
                }
            }
            Statement::FunctionCall(chain) => self.visit_suffixed(chain),
            Statement::DoBlock(do_block) => self.visit_block(&do_block.block),
            Statement::WhileLoop(while_loop) => {
                visit_nested_expr(self, &while_loop.condition);
                self.visit_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.visit_block(&repeat_loop.block);
                visit_nested_expr(self, &repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                visit_nested_expr(self, &if_stmt.condition);
                self.visit_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    visit_nested_expr(self, &clause.condition);
                    self.visit_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.visit_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                if let Some(loop_variable_type) = &numeric_for.type_annotation {
                    visit_nested_type(self, loop_variable_type);
                }
                visit_nested_expr(self, &numeric_for.start);
                visit_nested_expr(self, &numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    visit_nested_expr(self, step);
                }
                self.visit_block(&numeric_for.block);
            }
            Statement::GenericFor(generic_for) => {
                for binding in generic_for.names.iter() {
                    if let Some(binding_type) = &binding.type_annotation {
                        visit_nested_type(self, binding_type);
                    }
                }
                for expr in generic_for.exprs.iter() {
                    visit_nested_expr(self, expr);
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
                        visit_nested_expr(self, expr);
                    }
                }
            }
            // Luau
            Statement::CompoundAssignment(compound) => {
                self.visit_assignment_target(&compound.var);
                visit_nested_expr(self, &compound.expr);
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
                        visit_nested_expr(self, expr);
                    }
                }
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                if let Some(generics) = &type_declaration.generics {
                    self.visit_generic_type_list(generics);
                }
                match &type_declaration.type_value {
                    TypeDeclarationValue::Alias(alias_type) => {
                        visit_nested_type(self, alias_type);
                    }
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
                    visit_nested_expr(self, expr);
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
            | Expression::Name(_)
            | Expression::Error(_) => {}
            Expression::FunctionDef(function_def) => {
                self.visit_function_body(&function_def.body);
            }
            Expression::Suffixed(chain) => self.visit_suffixed(chain),
            Expression::Parenthesized(parenthesized) => {
                visit_nested_expr(self, &parenthesized.expr);
            }
            Expression::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            Expression::BinaryOp(run) => {
                visit_operand(self, &run.first, first_operand_needs_parens(run));
                let precedence = run.precedence().0;
                let last = run.links.len().saturating_sub(1);
                for (i, link) in run.links.iter().enumerate() {
                    let needs_parens =
                        link_operand_needs_parens(precedence, &link.operand, i == last);
                    visit_nested_operand(self, &link.operand, needs_parens);
                }
            }
            Expression::UnaryOp(unary_op) => {
                let needs_parens = unary_operand_needs_parens(&unary_op.operand);
                visit_nested_operand(self, &unary_op.operand, needs_parens);
            }
            // Luau
            Expression::IfExpression(if_expr) => {
                visit_nested_expr(self, &if_expr.condition);
                visit_nested_expr(self, &if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    visit_nested_expr(self, &clause.condition);
                    visit_nested_expr(self, &clause.expr);
                }
                visit_nested_expr(self, &if_expr.else_expr);
            }
            // Luau
            Expression::InterpolatedString(interpolated_string) => {
                for segment in &interpolated_string.segments {
                    if let Some(interpolated) = &segment.expr {
                        visit_nested_expr(self, interpolated);
                    }
                }
            }
            // Luau
            Expression::TypeCast(cast) => {
                visit_operand(self, &cast.expr, cast_operand_needs_parens(&cast.expr));
                visit_nested_type(self, &cast.type_annotation);
            }
        }
    }

    fn walk_assignment_target(&mut self, target: &'ast Var) {
        match target {
            Var::Name(_) => {}
            Var::Suffixed(chain) => self.visit_suffixed(chain),
        }
    }

    fn walk_suffixed(&mut self, chain: &'ast SuffixedExpression) {
        visit_operand(self, &chain.prefix, !is_prefix_expr(&chain.prefix));
        for (i, suffix) in chain.suffixes.iter().enumerate() {
            match suffix {
                Suffix::Field(_) => {}
                Suffix::Index { index, .. } => visit_nested_expr(self, index),
                Suffix::Call(call) => self.visit_call(Call::new(chain.head(i), call)),
                // Luau
                Suffix::TypeInstantiation { type_args, .. } => {
                    for type_arg in type_args.args.iter() {
                        visit_nested_type(self, type_arg);
                    }
                }
            }
        }
    }

    /// Explicit type arguments, then the arguments.
    fn walk_call(&mut self, call: Call<'ast>) {
        if let Some(type_args) = call.explicit_type_args {
            for type_arg in type_args.args.iter() {
                visit_nested_type(self, type_arg);
            }
        }
        self.walk_function_args(call.args);
    }

    fn walk_function_args(&mut self, args: &'ast FunctionArgs) {
        match args {
            FunctionArgs::Parenthesized { args: exprs, .. } => {
                for expr in exprs.iter() {
                    visit_nested_expr(self, expr);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                self.visit_table_constructor(table);
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::Error(_) => {}
        }
    }

    fn walk_table_constructor(&mut self, table: &'ast TableConstructor) {
        for field in table.fields.iter() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    visit_nested_expr(self, key);
                    visit_nested_expr(self, value);
                }
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    visit_nested_expr(self, value);
                }
            }
        }
    }

    fn walk_function_body(&mut self, body: &'ast FunctionBody) {
        if let Some(generics) = &body.generics {
            self.visit_generic_type_list(generics);
        }
        for param in body.params.iter() {
            if let Some(param_type) = &param.type_annotation {
                visit_nested_type(self, param_type);
            }
        }
        if let Some(vararg) = &body.vararg
            && let Some(vararg_type) = &vararg.type_annotation
        {
            visit_nested_type(self, vararg_type);
        }
        if let Some(return_type) = &body.return_type {
            visit_nested_type(self, return_type);
        }
        self.visit_block(&body.block);
    }

    fn walk_type(&mut self, type_value: &'ast Type) {
        match type_value {
            Type::Named(named) => {
                if let Some(generics) = &named.generics {
                    for arg in generics.args.iter() {
                        visit_nested_type(self, arg);
                    }
                }
            }
            // typeof embeds a real expression, so semantic passes must see it.
            Type::Typeof(typeof_type) => visit_nested_expr(self, &typeof_type.expr),
            Type::Table(table) => {
                for field in table.fields.iter() {
                    match field {
                        TypeField::Named { value, .. } | TypeField::Array { value, .. } => {
                            visit_nested_type(self, value);
                        }
                        TypeField::Indexer { key, value, .. } => {
                            visit_nested_type(self, key);
                            visit_nested_type(self, value);
                        }
                    }
                }
            }
            Type::Function(function_type) => {
                if let Some(generics) = &function_type.generics {
                    self.visit_generic_type_list(generics);
                }
                for param in function_type.params.iter() {
                    visit_nested_type(self, &param.type_value);
                }
                visit_nested_type(self, &function_type.return_type);
            }
            Type::Optional(optional) => {
                let needs_parens = optional_inner_needs_parens(&optional.type_value);
                visit_type_operand(self, &optional.type_value, needs_parens);
            }
            Type::Union(union) => {
                let last = union.types.len().saturating_sub(1);
                for (i, member) in union.types.iter().enumerate() {
                    let needs_parens = union_member_needs_parens(member, i == last);
                    visit_type_operand(self, member, needs_parens);
                }
            }
            Type::Intersection(intersection) => {
                let last = intersection.types.len().saturating_sub(1);
                for (i, member) in intersection.types.iter().enumerate() {
                    let needs_parens = intersection_member_needs_parens(member, i == last);
                    visit_type_operand(self, member, needs_parens);
                }
            }
            Type::Parenthesized(parenthesized) => {
                visit_nested_type(self, &parenthesized.type_value);
            }
            Type::Pack(pack) => {
                for item in pack.types.iter() {
                    visit_nested_type(self, item);
                }
            }
            Type::Variadic(variadic) => visit_nested_type(self, &variadic.type_value),
            Type::Name { .. } | Type::Singleton(_) | Type::GenericPack(_) | Type::Error(_) => {}
        }
    }

    fn walk_generic_type_list(&mut self, generics: &'ast GenericTypeList) {
        for param in generics.params.iter() {
            if let Some(default) = &param.default {
                visit_nested_type(self, default);
            }
        }
    }

    fn walk_attributed_names(&mut self, names: &'ast Punctuated<AttributedName>) {
        for name in names.iter() {
            if let Some(name_type) = &name.type_annotation {
                visit_nested_type(self, name_type);
            }
        }
    }
}

/// Visit `expr` a level down. An error stands for nothing the parser read,
/// so it opens none.
fn visit_nested_expr<'ast, V>(visitor: &mut V, expr: &'ast Expression)
where
    V: Visitor<'ast> + ?Sized,
{
    if matches!(expr, Expression::Error(_)) {
        visitor.visit_expr(expr);
        return;
    }
    if visitor.descend() {
        visitor.visit_expr(expr);
    }
    visitor.ascend();
}

/// Visit `expr` on its parent's level, or a level down where it needs
/// parentheses.
fn visit_operand<'ast, V>(visitor: &mut V, expr: &'ast Expression, needs_parens: bool)
where
    V: Visitor<'ast> + ?Sized,
{
    if needs_parens {
        visit_nested_expr(visitor, expr);
    } else {
        visitor.visit_expr(expr);
    }
}

/// Visit `expr` a level down, and one more where it needs parentheses.
fn visit_nested_operand<'ast, V>(visitor: &mut V, expr: &'ast Expression, needs_parens: bool)
where
    V: Visitor<'ast> + ?Sized,
{
    if !needs_parens {
        visit_nested_expr(visitor, expr);
        return;
    }
    if visitor.descend() {
        visit_nested_expr(visitor, expr);
    }
    visitor.ascend();
}

/// As [`visit_nested_expr`], for a type.
fn visit_nested_type<'ast, V>(visitor: &mut V, type_value: &'ast Type)
where
    V: Visitor<'ast> + ?Sized,
{
    if matches!(type_value, Type::Error(_)) {
        visitor.visit_type(type_value);
        return;
    }
    if visitor.descend() {
        visitor.visit_type(type_value);
    }
    visitor.ascend();
}

/// As [`visit_operand`], for a type.
fn visit_type_operand<'ast, V>(visitor: &mut V, type_value: &'ast Type, needs_parens: bool)
where
    V: Visitor<'ast> + ?Sized,
{
    if needs_parens {
        visit_nested_type(visitor, type_value);
    } else {
        visitor.visit_type(type_value);
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
        fn visit_call(&mut self, call: Call<'ast>) {
            self.calls = self.calls.saturating_add(1);
            self.walk_call(call);
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
