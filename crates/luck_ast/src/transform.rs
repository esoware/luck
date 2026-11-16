use core::mem;

use luck_token::{Span, Token, TokenKind};

use crate::depth::{
    cast_operand_needs_parens, first_operand_needs_parens, intersection_member_needs_parens,
    link_operand_needs_parens, optional_inner_needs_parens, unary_operand_needs_parens,
    union_member_needs_parens,
};
use crate::expr::{
    CallSuffix, Expression, Field, FunctionArgs, ParenthesizedExpression, Suffix,
    SuffixedExpression, TableConstructor, Var,
};
use crate::function::{FunctionBody, Parameter};
use crate::punctuated::Punctuated;
use crate::query::is_prefix_expr;
use crate::stmt::{
    AttributedName, Block, FunctionDecl, GlobalFunction, IfStatement, LastStatement, LocalFunction,
    Statement, TypeDeclaration, TypeDeclarationValue,
};
use crate::types::{GenericTypeList, Type, TypeField};

/// Each `transform_*` hook defaults to its `walk_*`, which rebuilds the node
/// from its transformed children.
///
/// Override the hooks you need and call `self.walk_*` from them to recurse. The `walk_*` methods
/// are the default recursion, so overriding one instead of its hook would leave nothing to fall
/// back on.
pub trait Transform {
    /// Called by the walkers on entering each level of nesting, the levels
    /// [`Visitor::descend`](crate::Visitor::descend) counts, and always
    /// paired with [`Transform::ascend`].
    fn descend(&mut self) {}
    fn ascend(&mut self) {}

    fn transform_block(&mut self, block: Block) -> Block {
        self.walk_block(block)
    }
    fn transform_stmt(&mut self, stmt: Statement) -> Statement {
        self.walk_stmt(stmt)
    }
    fn transform_last_stmt(&mut self, last: LastStatement) -> LastStatement {
        self.walk_last_stmt(last)
    }
    fn transform_expr(&mut self, expr: Expression) -> Expression {
        self.walk_expr(expr)
    }
    /// Reached from assignments and Luau compound assignments. A name read
    /// as a value is an [`Expression::Name`] instead.
    fn transform_assignment_target(&mut self, target: Var) -> Var {
        self.walk_assignment_target(target)
    }
    /// Reached from a call statement, an expression, and an assignment
    /// target. The chain stays boxed, so a pass moves a pointer, not the
    /// node.
    #[expect(clippy::unnecessary_box_returns, reason = "the chain goes back into its own box")]
    fn transform_suffixed(&mut self, chain: Box<SuffixedExpression>) -> Box<SuffixedExpression> {
        self.walk_suffixed(chain)
    }
    /// Reached once for each call in a chain.
    fn transform_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        self.walk_function_args(args)
    }
    /// Reached from both a table expression and a `f{...}` argument.
    fn transform_table_constructor(&mut self, table: TableConstructor) -> TableConstructor {
        self.walk_table_constructor(table)
    }
    fn transform_function_body(&mut self, body: FunctionBody) -> FunctionBody {
        self.walk_function_body(body)
    }
    fn transform_type(&mut self, type_value: Type) -> Type {
        self.walk_type(type_value)
    }

    fn walk_block(&mut self, mut block: Block) -> Block {
        for stmt in &mut block.stmts {
            self.descend();
            *stmt = self.transform_stmt(stmt.take());
            self.ascend();
        }
        if let Some(last) = &mut block.last_stmt {
            self.descend();
            **last = self.transform_last_stmt(last.take());
            self.ascend();
        }
        block
    }

    fn walk_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Assignment(mut assignment) => {
                for target in &mut assignment.targets.items {
                    let placeholder = Var::Name(Token::new(TokenKind::Eof, Span::default()));
                    *target = self.transform_assignment_target(mem::replace(target, placeholder));
                }
                walk_exprs(self, &mut assignment.values.items);
                Statement::Assignment(assignment)
            }
            Statement::FunctionCall(chain) => {
                Statement::FunctionCall(self.transform_suffixed(chain))
            }
            Statement::DoBlock(mut do_block) => {
                do_block.block = self.transform_block(do_block.block);
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = transform_nested_expr(self, while_loop.condition);
                while_loop.block = self.transform_block(while_loop.block);
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                repeat_loop.block = self.transform_block(repeat_loop.block);
                repeat_loop.condition = transform_nested_expr(self, repeat_loop.condition);
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(if_stmt) => Statement::IfStatement(self.walk_if_stmt(if_stmt)),
            Statement::NumericFor(mut numeric_for) => {
                if let Some(loop_variable_type) = &mut numeric_for.type_annotation {
                    *loop_variable_type = transform_nested_type(self, loop_variable_type.take());
                }
                numeric_for.start = transform_nested_expr(self, numeric_for.start);
                numeric_for.limit = transform_nested_expr(self, numeric_for.limit);
                if let Some(step) = &mut numeric_for.step {
                    *step = transform_nested_expr(self, step.take());
                }
                numeric_for.block = self.transform_block(numeric_for.block);
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                walk_params(self, &mut generic_for.names.items);
                walk_exprs(self, &mut generic_for.exprs.items);
                generic_for.block = self.transform_block(generic_for.block);
                Statement::GenericFor(generic_for)
            }
            Statement::FunctionDecl(function_decl) => walk_function_decl(self, function_decl),
            Statement::LocalFunction(local_function) => walk_local_function(self, local_function),
            Statement::LocalAssignment(mut local_assignment) => {
                walk_names(self, &mut local_assignment.names.items);
                if let Some(exprs) = &mut local_assignment.exprs {
                    walk_exprs(self, &mut exprs.items);
                }
                Statement::LocalAssignment(local_assignment)
            }
            // Luau
            Statement::CompoundAssignment(mut compound) => {
                compound.var = self.transform_assignment_target(compound.var);
                compound.expr = transform_nested_expr(self, compound.expr);
                Statement::CompoundAssignment(compound)
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                walk_global_function(self, global_function)
            }
            // Lua 5.5
            Statement::GlobalDeclaration(mut global_declaration) => {
                walk_names(self, &mut global_declaration.names.items);
                if let Some(exprs) = &mut global_declaration.exprs {
                    walk_exprs(self, &mut exprs.items);
                }
                Statement::GlobalDeclaration(global_declaration)
            }
            // Luau
            Statement::TypeDeclaration(type_declaration) => {
                walk_type_declaration(self, type_declaration)
            }
            leaf @ (Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_)) => leaf,
        }
    }

    fn walk_if_stmt(&mut self, mut if_stmt: Box<IfStatement>) -> Box<IfStatement> {
        if_stmt.condition = transform_nested_expr(self, if_stmt.condition);
        if_stmt.block = self.transform_block(if_stmt.block);
        for clause in &mut if_stmt.elseif_clauses {
            clause.condition = transform_nested_expr(self, clause.condition.take());
            clause.block = self.transform_block(clause.block.take());
        }
        if let Some(clause) = &mut if_stmt.else_clause {
            clause.block = self.transform_block(clause.block.take());
        }
        if_stmt
    }

    fn walk_last_stmt(&mut self, last: LastStatement) -> LastStatement {
        match last {
            LastStatement::Return(mut return_stmt) => {
                walk_exprs(self, &mut return_stmt.exprs.items);
                LastStatement::Return(return_stmt)
            }
            leaf @ (LastStatement::Break(_)
            | LastStatement::Continue(_)
            | LastStatement::Error(_)) => leaf,
        }
    }

    fn walk_expr(&mut self, expr: Expression) -> Expression {
        match expr {
            leaf @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Name(_)
            | Expression::Error(_)) => leaf,
            Expression::FunctionDef(mut function_def) => {
                function_def.body = self.transform_function_body(function_def.body);
                Expression::FunctionDef(function_def)
            }
            Expression::Suffixed(chain) => Expression::Suffixed(self.transform_suffixed(chain)),
            Expression::Parenthesized(parenthesized) => walk_parenthesized(self, parenthesized),
            Expression::TableConstructor(table) => {
                Expression::TableConstructor(walk_boxed_table(self, table))
            }
            Expression::BinaryOp(mut run) => {
                let first_needs_parens = first_operand_needs_parens(&run);
                run.first = transform_operand(self, run.first, first_needs_parens);
                let precedence = run.precedence().0;
                let last = run.links.len().saturating_sub(1);
                for (i, link) in run.links.iter_mut().enumerate() {
                    let needs_parens =
                        link_operand_needs_parens(precedence, &link.operand, i == last);
                    link.operand =
                        transform_nested_operand(self, link.operand.take(), needs_parens);
                }
                Expression::BinaryOp(run)
            }
            Expression::UnaryOp(mut unary_op) => {
                let needs_parens = unary_operand_needs_parens(&unary_op.operand);
                unary_op.operand = transform_nested_operand(self, unary_op.operand, needs_parens);
                Expression::UnaryOp(unary_op)
            }
            // Luau
            Expression::IfExpression(mut if_expr) => {
                if_expr.condition = transform_nested_expr(self, if_expr.condition);
                if_expr.then_expr = transform_nested_expr(self, if_expr.then_expr);
                for clause in &mut if_expr.elseif_clauses {
                    clause.condition = transform_nested_expr(self, clause.condition.take());
                    clause.expr = transform_nested_expr(self, clause.expr.take());
                }
                if_expr.else_expr = transform_nested_expr(self, if_expr.else_expr);
                Expression::IfExpression(if_expr)
            }
            // Luau
            Expression::InterpolatedString(mut interpolated_string) => {
                for segment in &mut interpolated_string.segments {
                    if let Some(interpolated) = &mut segment.expr {
                        *interpolated = transform_nested_expr(self, interpolated.take());
                    }
                }
                Expression::InterpolatedString(interpolated_string)
            }
            // Luau
            Expression::TypeCast(mut cast) => {
                let needs_parens = cast_operand_needs_parens(&cast.expr);
                cast.expr = transform_operand(self, cast.expr, needs_parens);
                cast.type_annotation = transform_nested_type(self, cast.type_annotation);
                Expression::TypeCast(cast)
            }
        }
    }

    fn walk_assignment_target(&mut self, target: Var) -> Var {
        match target {
            Var::Name(_) => target,
            Var::Suffixed(chain) => Var::Suffixed(self.transform_suffixed(chain)),
        }
    }

    /// Rewrites the prefix, then each suffix in source order, as the
    /// `Visitor` walk reaches them: an index, a call's explicit type
    /// arguments and then its arguments, or a Luau instantiation's type
    /// arguments.
    #[expect(clippy::unnecessary_box_returns, reason = "the chain goes back into its own box")]
    fn walk_suffixed(&mut self, mut chain: Box<SuffixedExpression>) -> Box<SuffixedExpression> {
        let needs_parens = !is_prefix_expr(&chain.prefix);
        chain.prefix = transform_operand(self, chain.prefix, needs_parens);
        for suffix in &mut chain.suffixes {
            match suffix {
                Suffix::Field(_) => {}
                Suffix::Index { index, .. } => *index = transform_nested_expr(self, index.take()),
                Suffix::Call(call) => {
                    let args = match call {
                        CallSuffix::Plain(args) => args,
                        CallSuffix::Method(method) => {
                            if let Some(type_args) = &mut method.explicit_type_args {
                                walk_types(self, &mut type_args.args.items);
                            }
                            &mut method.args
                        }
                    };
                    *args = self.transform_function_args(args.take());
                }
                // Luau
                Suffix::TypeInstantiation { type_args, .. } => {
                    walk_types(self, &mut type_args.args.items);
                }
            }
        }
        chain
    }

    fn walk_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        match args {
            FunctionArgs::Parenthesized { span, args: mut exprs } => {
                walk_exprs(self, &mut exprs.items);
                FunctionArgs::Parenthesized { span, args: exprs }
            }
            FunctionArgs::TableConstructor(table) => {
                FunctionArgs::TableConstructor(walk_boxed_table(self, table))
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::Error(_) => args,
        }
    }

    fn walk_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        for field in &mut table.fields.items {
            match field {
                Field::Bracketed { key, value, .. } => {
                    *key = transform_nested_expr(self, key.take());
                    *value = transform_nested_expr(self, value.take());
                }
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    *value = transform_nested_expr(self, value.take());
                }
            }
        }
        table
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        if let Some(generics) = &mut body.generics {
            walk_generic_params(self, generics);
        }
        walk_params(self, &mut body.params.items);
        if let Some(vararg) = &mut body.vararg
            && let Some(vararg_type) = &mut vararg.type_annotation
        {
            *vararg_type = transform_nested_type(self, vararg_type.take());
        }
        if let Some(return_type) = &mut body.return_type {
            *return_type = transform_nested_type(self, return_type.take());
        }
        body.block = self.transform_block(body.block);
        body
    }

    fn walk_type(&mut self, type_value: Type) -> Type {
        match type_value {
            Type::Named(mut named) => {
                if let Some(generics) = &mut named.generics {
                    walk_types(self, &mut generics.args.items);
                }
                Type::Named(named)
            }
            // typeof embeds a real expression, so semantic passes must see it.
            Type::Typeof(mut typeof_type) => {
                typeof_type.expr = transform_nested_expr(self, typeof_type.expr);
                Type::Typeof(typeof_type)
            }
            Type::Table(mut table) => {
                for field in &mut table.fields.items {
                    match field {
                        TypeField::Named { value, .. } | TypeField::Array { value, .. } => {
                            *value = transform_nested_type(self, value.take());
                        }
                        TypeField::Indexer { key, value, .. } => {
                            *key = transform_nested_type(self, key.take());
                            *value = transform_nested_type(self, value.take());
                        }
                    }
                }
                Type::Table(table)
            }
            Type::Function(mut function_type) => {
                if let Some(generics) = &mut function_type.generics {
                    walk_generic_params(self, generics);
                }
                for param in &mut function_type.params.items {
                    param.type_value = transform_nested_type(self, param.type_value.take());
                }
                function_type.return_type = transform_nested_type(self, function_type.return_type);
                Type::Function(function_type)
            }
            Type::Optional(mut optional) => {
                let needs_parens = optional_inner_needs_parens(&optional.type_value);
                optional.type_value =
                    transform_type_operand(self, optional.type_value, needs_parens);
                Type::Optional(optional)
            }
            Type::Union(mut union) => {
                let last = union.types.items.len().saturating_sub(1);
                for (i, member) in union.types.items.iter_mut().enumerate() {
                    let needs_parens = union_member_needs_parens(member, i == last);
                    *member = transform_type_operand(self, member.take(), needs_parens);
                }
                Type::Union(union)
            }
            Type::Intersection(mut intersection) => {
                let last = intersection.types.items.len().saturating_sub(1);
                for (i, member) in intersection.types.items.iter_mut().enumerate() {
                    let needs_parens = intersection_member_needs_parens(member, i == last);
                    *member = transform_type_operand(self, member.take(), needs_parens);
                }
                Type::Intersection(intersection)
            }
            Type::Parenthesized(mut parenthesized) => {
                parenthesized.type_value = transform_nested_type(self, parenthesized.type_value);
                Type::Parenthesized(parenthesized)
            }
            Type::Pack(mut pack) => {
                walk_types(self, &mut pack.types.items);
                Type::Pack(pack)
            }
            Type::Variadic(mut variadic) => {
                variadic.type_value = transform_nested_type(self, variadic.type_value);
                Type::Variadic(variadic)
            }
            leaf @ (Type::Name { .. }
            | Type::Singleton(_)
            | Type::GenericPack(_)
            | Type::Error(_)) => leaf,
        }
    }

    fn walk_generic_type_list(&mut self, mut generics: GenericTypeList) -> GenericTypeList {
        walk_generic_params(self, &mut generics);
        generics
    }

    fn walk_attributed_names(
        &mut self,
        mut names: Punctuated<AttributedName>,
    ) -> Punctuated<AttributedName> {
        walk_names(self, &mut names.items);
        names
    }

    fn walk_punctuated_exprs(
        &mut self,
        mut exprs: Punctuated<Expression>,
    ) -> Punctuated<Expression> {
        walk_exprs(self, &mut exprs.items);
        exprs
    }
}

/// Rewrite `expr` a level down. An error stands for nothing the parser read,
/// so it opens none.
fn transform_nested_expr<T>(transform: &mut T, expr: Expression) -> Expression
where
    T: Transform + ?Sized,
{
    if matches!(expr, Expression::Error(_)) {
        return transform.transform_expr(expr);
    }
    transform.descend();
    let transformed = transform.transform_expr(expr);
    transform.ascend();
    transformed
}

/// Rewrite `expr` on its parent's level, or a level down where it needs
/// parentheses.
fn transform_operand<T>(transform: &mut T, expr: Expression, needs_parens: bool) -> Expression
where
    T: Transform + ?Sized,
{
    if needs_parens {
        transform_nested_expr(transform, expr)
    } else {
        transform.transform_expr(expr)
    }
}

/// Rewrite `expr` a level down, and one more where it needs parentheses.
fn transform_nested_operand<T>(
    transform: &mut T,
    expr: Expression,
    needs_parens: bool,
) -> Expression
where
    T: Transform + ?Sized,
{
    if !needs_parens {
        return transform_nested_expr(transform, expr);
    }
    transform.descend();
    let transformed = transform_nested_expr(transform, expr);
    transform.ascend();
    transformed
}

/// As [`transform_nested_expr`], for a type.
fn transform_nested_type<T>(transform: &mut T, type_value: Type) -> Type
where
    T: Transform + ?Sized,
{
    if matches!(type_value, Type::Error(_)) {
        return transform.transform_type(type_value);
    }
    transform.descend();
    let transformed = transform.transform_type(type_value);
    transform.ascend();
    transformed
}

/// As [`transform_operand`], for a type.
fn transform_type_operand<T>(transform: &mut T, type_value: Type, needs_parens: bool) -> Type
where
    T: Transform + ?Sized,
{
    if needs_parens {
        transform_nested_type(transform, type_value)
    } else {
        transform.transform_type(type_value)
    }
}

fn walk_exprs<T>(transform: &mut T, exprs: &mut [Expression])
where
    T: Transform + ?Sized,
{
    for expr in exprs {
        *expr = transform_nested_expr(transform, expr.take());
    }
}

fn walk_types<T>(transform: &mut T, types: &mut [Type])
where
    T: Transform + ?Sized,
{
    for type_value in types {
        *type_value = transform_nested_type(transform, type_value.take());
    }
}

// Moving a function body through `transform_function_body` reserves stack
// for two copies of it, so the kinds that hold one rewrite in a frame of
// their own, off the stack under every statement nested in them. A
// parenthesized expression does too, for every expression nested in it.

#[inline(never)]
fn walk_function_decl<T>(transform: &mut T, mut function_decl: Box<FunctionDecl>) -> Statement
where
    T: Transform + ?Sized,
{
    function_decl.body = transform.transform_function_body(function_decl.body);
    Statement::FunctionDecl(function_decl)
}

#[inline(never)]
fn walk_local_function<T>(transform: &mut T, mut local_function: Box<LocalFunction>) -> Statement
where
    T: Transform + ?Sized,
{
    local_function.body = transform.transform_function_body(local_function.body);
    Statement::LocalFunction(local_function)
}

#[inline(never)]
fn walk_global_function<T>(transform: &mut T, mut global_function: Box<GlobalFunction>) -> Statement
where
    T: Transform + ?Sized,
{
    global_function.body = transform.transform_function_body(global_function.body);
    Statement::GlobalFunction(global_function)
}

#[inline(never)]
fn walk_type_declaration<T>(
    transform: &mut T,
    mut type_declaration: Box<TypeDeclaration>,
) -> Statement
where
    T: Transform + ?Sized,
{
    if let Some(generics) = &mut type_declaration.generics {
        walk_generic_params(transform, generics);
    }
    type_declaration.type_value = match type_declaration.type_value {
        TypeDeclarationValue::Alias(alias_type) => {
            TypeDeclarationValue::Alias(transform_nested_type(transform, alias_type))
        }
        TypeDeclarationValue::TypeFunction(mut body) => {
            *body = transform.transform_function_body(*body);
            TypeDeclarationValue::TypeFunction(body)
        }
    };
    Statement::TypeDeclaration(type_declaration)
}

#[inline(never)]
fn walk_parenthesized<T>(
    transform: &mut T,
    mut parenthesized: Box<ParenthesizedExpression>,
) -> Expression
where
    T: Transform + ?Sized,
{
    parenthesized.expr = transform_nested_expr(transform, parenthesized.expr);
    Expression::Parenthesized(parenthesized)
}

fn walk_params<T>(transform: &mut T, params: &mut [Parameter])
where
    T: Transform + ?Sized,
{
    for param in params {
        if let Some(param_type) = &mut param.type_annotation {
            *param_type = transform_nested_type(transform, param_type.take());
        }
    }
}

fn walk_names<T>(transform: &mut T, names: &mut [AttributedName])
where
    T: Transform + ?Sized,
{
    for name in names {
        if let Some(name_type) = &mut name.type_annotation {
            *name_type = transform_nested_type(transform, name_type.take());
        }
    }
}

fn walk_generic_params<T>(transform: &mut T, generics: &mut GenericTypeList)
where
    T: Transform + ?Sized,
{
    for param in &mut generics.params.items {
        if let Some(default) = &mut param.default {
            *default = transform_nested_type(transform, default.take());
        }
    }
}

fn walk_boxed_table<T>(transform: &mut T, mut table: Box<TableConstructor>) -> Box<TableConstructor>
where
    T: Transform + ?Sized,
{
    *table = transform.transform_table_constructor(*table);
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Synth;
    use crate::visitor::Visitor;

    struct NilToFalse;

    impl Transform for NilToFalse {
        fn transform_expr(&mut self, expr: Expression) -> Expression {
            let walked = self.walk_expr(expr);
            if let Expression::Nil(nil_span) = walked {
                Expression::False(nil_span)
            } else {
                walked
            }
        }
    }

    #[derive(Debug, Default, PartialEq, Eq)]
    struct NilFalseCounts {
        nils: usize,
        falses: usize,
    }

    impl<'ast> Visitor<'ast> for NilFalseCounts {
        fn visit_expr(&mut self, expr: &'ast Expression) {
            match expr {
                Expression::Nil(_) => self.nils = self.nils.saturating_add(1),
                Expression::False(_) => self.falses = self.falses.saturating_add(1),
                Expression::True(_)
                | Expression::Number(_)
                | Expression::Integer(_)
                | Expression::StringLiteral(_)
                | Expression::VarArg(_)
                | Expression::FunctionDef(_)
                | Expression::Name(_)
                | Expression::Suffixed(_)
                | Expression::Parenthesized(_)
                | Expression::TableConstructor(_)
                | Expression::BinaryOp(_)
                | Expression::UnaryOp(_)
                | Expression::IfExpression(_)
                | Expression::InterpolatedString(_)
                | Expression::TypeCast(_)
                | Expression::Error(_) => {}
            }
            self.walk_expr(expr);
        }
    }

    fn rewrite_counts(stmts: Vec<Statement>) -> NilFalseCounts {
        let synth = Synth::new();
        let transformed = NilToFalse.transform_block(synth.block(stmts, None));
        let mut counts = NilFalseCounts::default();
        counts.visit_block(&transformed);
        counts
    }

    fn all_false(falses: usize) -> NilFalseCounts {
        NilFalseCounts { nils: 0, falses }
    }

    #[test]
    fn transform_reaches_local_initializers() {
        // local x = nil
        let synth = Synth::new();
        let stmts = vec![synth.local(&["x"], vec![synth.nil()])];
        assert_eq!(rewrite_counts(stmts), all_false(1));
    }

    #[test]
    fn transform_reaches_table_fields() {
        // local t = {nil, nil}
        let synth = Synth::new();
        let table = synth.array(vec![synth.nil(), synth.nil()]);
        assert_eq!(rewrite_counts(vec![synth.local(&["t"], vec![table])]), all_false(2));
    }

    #[test]
    fn transform_reaches_function_returns() {
        // local function f() return nil end
        let synth = Synth::new();
        let body = synth.block(vec![], Some(synth.return_(vec![synth.nil()])));
        assert_eq!(rewrite_counts(vec![synth.local_function("f", &[], body)]), all_false(1));
    }

    #[test]
    fn transform_reaches_if_conditions_and_branches() {
        // if nil then x = nil else y = nil end
        let synth = Synth::new();
        let then_block =
            synth.block(vec![synth.assign(vec![synth.name_var("x")], vec![synth.nil()])], None);
        let else_block =
            synth.block(vec![synth.assign(vec![synth.name_var("y")], vec![synth.nil()])], None);
        let if_stmt = synth.if_(synth.nil(), then_block, vec![], Some(else_block));
        assert_eq!(rewrite_counts(vec![if_stmt]), all_false(3));
    }

    #[test]
    fn default_transform_returns_the_tree_unchanged() {
        struct Identity;
        impl Transform for Identity {}

        // local x = 1 + 2; a.b, c = x, x
        let synth = Synth::new();
        let sum = synth.binary_op(synth.number("1"), luck_token::BinOp::Add, synth.number("2"));
        let targets = vec![synth.field_var(synth.name_expr("a"), "b"), synth.name_var("c")];
        let values = vec![synth.name_expr("x"), synth.name_expr("x")];
        let block =
            synth.block(vec![synth.local(&["x"], vec![sum]), synth.assign(targets, values)], None);
        let original = block.clone();
        assert_eq!(Identity.transform_block(block), original);
    }

    #[test]
    fn transform_reaches_while_conditions() {
        // while nil do end
        let synth = Synth::new();
        let while_loop = synth.while_(synth.nil(), synth.block(vec![], None));
        assert_eq!(rewrite_counts(vec![while_loop]), all_false(1));
    }

    #[test]
    fn transform_reaches_repeat_conditions() {
        // repeat until nil
        let synth = Synth::new();
        let repeat_loop = synth.repeat_(synth.block(vec![], None), synth.nil());
        assert_eq!(rewrite_counts(vec![repeat_loop]), all_false(1));
    }

    #[test]
    fn transform_reaches_numeric_for_bounds_and_step() {
        // for i = nil, nil, nil do end
        let synth = Synth::new();
        let numeric_for = synth.numeric_for(
            synth.param("i"),
            synth.nil(),
            synth.nil(),
            Some(synth.nil()),
            synth.block(vec![], None),
        );
        assert_eq!(rewrite_counts(vec![numeric_for]), all_false(3));
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

        impl Transform for HookCounts {
            fn transform_suffixed(
                &mut self,
                chain: Box<SuffixedExpression>,
            ) -> Box<SuffixedExpression> {
                self.chains = self.chains.saturating_add(1);
                self.walk_suffixed(chain)
            }
            fn transform_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
                self.args = self.args.saturating_add(1);
                self.walk_function_args(args)
            }
            fn transform_table_constructor(&mut self, table: TableConstructor) -> TableConstructor {
                self.tables = self.tables.saturating_add(1);
                self.walk_table_constructor(table)
            }
            fn transform_function_body(&mut self, body: FunctionBody) -> FunctionBody {
                self.bodies = self.bodies.saturating_add(1);
                self.walk_function_body(body)
            }
        }

        // f{}(function() end); local t = {}
        let synth = Synth::new();
        let table_call = synth.call_table(synth.name_expr("f"), vec![]);
        let function = synth.function_def(&[], synth.block(vec![], None));
        let block = synth.block(
            vec![
                synth.call_stmt(synth.call(table_call, vec![function])),
                synth.local(&["t"], vec![synth.table(vec![])]),
            ],
            None,
        );
        let mut counts = HookCounts::default();
        counts.transform_block(block);
        assert_eq!(counts, HookCounts { chains: 1, args: 2, tables: 2, bodies: 1 });
    }
}
