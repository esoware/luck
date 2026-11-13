use crate::depth::regrouped_levels;
use crate::expr::{
    BinaryOp, Expression, Field, FunctionArgs, FunctionCall, FunctionDef, IfExpression,
    InterpolatedString, ParenthesizedExpression, TableConstructor, TypeCast, TypeInstantiation,
    UnaryOp, Var,
};
use crate::function::{FunctionBody, Parameter};
use crate::punctuated::Punctuated;
use crate::stmt::{
    Assignment, AttributedName, Block, CompoundAssignment, DoBlock, FunctionDecl, GenericFor,
    GlobalDeclaration, GlobalFunction, IfStatement, LastStatement, LocalAssignment, LocalFunction,
    NumericFor, RepeatLoop, Statement, TypeDeclaration, TypeDeclarationValue, WhileLoop,
};
use crate::types::{
    FunctionType, GenericTypeList, NamedType, TableType, Type, TypeArgs, TypeField, TypeofType,
};

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
    fn transform_var(&mut self, var: Var) -> Var {
        self.walk_var(var)
    }
    /// Reached from both a call statement and a call expression.
    fn transform_function_call(&mut self, call: FunctionCall) -> FunctionCall {
        self.walk_function_call(call)
    }
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
        self.descend();
        for stmt in &mut block.stmts {
            *stmt = self.transform_stmt(stmt.take());
        }
        if let Some(last) = &mut block.last_stmt {
            **last = self.transform_last_stmt(last.take());
        }
        self.ascend();
        block
    }

    // Each arm rebuilds its node in a function of its own, so a level of
    // nesting holds only that node's temporaries on the stack.
    fn walk_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Assignment(assignment) => walk_assignment(self, assignment),
            Statement::FunctionCall(call) => Statement::FunctionCall(walk_boxed_call(self, call)),
            Statement::DoBlock(do_block) => walk_do_block(self, do_block),
            Statement::WhileLoop(while_loop) => walk_while_loop(self, while_loop),
            Statement::RepeatLoop(repeat_loop) => walk_repeat_loop(self, repeat_loop),
            Statement::IfStatement(if_stmt) => Statement::IfStatement(self.walk_if_stmt(if_stmt)),
            Statement::NumericFor(numeric_for) => walk_numeric_for(self, numeric_for),
            Statement::GenericFor(generic_for) => walk_generic_for(self, generic_for),
            Statement::FunctionDecl(function_decl) => walk_function_decl(self, function_decl),
            Statement::LocalFunction(local_function) => walk_local_function(self, local_function),
            Statement::LocalAssignment(local_assignment) => {
                walk_local_assignment(self, local_assignment)
            }
            // Luau
            Statement::CompoundAssignment(compound) => walk_compound_assignment(self, compound),
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                walk_global_function(self, global_function)
            }
            // Lua 5.5
            Statement::GlobalDeclaration(global_declaration) => {
                walk_global_declaration(self, global_declaration)
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
        if_stmt.condition = self.transform_expr(if_stmt.condition);
        if_stmt.block = self.transform_block(if_stmt.block);
        for clause in &mut if_stmt.elseif_clauses {
            clause.condition = self.transform_expr(clause.condition.take());
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
            | Expression::Error(_)) => leaf,
            Expression::FunctionDef(function_def) => walk_function_def(self, function_def),
            Expression::Var(var) => Expression::Var(self.transform_var(var)),
            Expression::FunctionCall(call) => Expression::FunctionCall(walk_boxed_call(self, call)),
            Expression::Parenthesized(parenthesized) => walk_parenthesized(self, parenthesized),
            Expression::TableConstructor(table) => {
                Expression::TableConstructor(walk_boxed_table(self, table))
            }
            Expression::BinaryOp(binary_op) => walk_binary_op(self, binary_op),
            Expression::UnaryOp(unary_op) => walk_unary_op(self, unary_op),
            // Luau
            Expression::IfExpression(if_expr) => walk_if_expr(self, if_expr),
            // Luau
            Expression::InterpolatedString(interpolated_string) => {
                walk_interpolated_string(self, interpolated_string)
            }
            // Luau
            Expression::TypeCast(cast) => walk_type_cast(self, cast),
            // Luau
            Expression::TypeInstantiation(instantiation) => {
                walk_type_instantiation(self, instantiation)
            }
        }
    }

    fn walk_var(&mut self, var: Var) -> Var {
        match var {
            Var::Name(_) => var,
            Var::Index(mut index_expr) => {
                self.descend();
                index_expr.prefix = self.transform_expr(index_expr.prefix);
                index_expr.index = self.transform_expr(index_expr.index);
                self.ascend();
                Var::Index(index_expr)
            }
            Var::FieldAccess(mut field_access) => {
                self.descend();
                field_access.prefix = self.transform_expr(field_access.prefix);
                self.ascend();
                Var::FieldAccess(field_access)
            }
        }
    }

    /// Rewrites the callee, then any explicit type arguments, then the
    /// arguments, in source order as the `Visitor` walk reaches them.
    fn walk_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        self.descend();
        call.callee = self.transform_expr(call.callee);
        if let Some(type_args) = &mut call.explicit_type_args {
            walk_types(self, &mut type_args.args.items);
        }
        call.args = self.transform_function_args(call.args);
        self.ascend();
        call
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
            FunctionArgs::StringLiteral(_) => args,
        }
    }

    fn walk_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        self.descend();
        for field in &mut table.fields.items {
            match field {
                Field::Bracketed { key, value, .. } => {
                    *key = self.transform_expr(key.take());
                    *value = self.transform_expr(value.take());
                }
                Field::Named { value, .. } | Field::Positional { value, .. } => {
                    *value = self.transform_expr(value.take());
                }
            }
        }
        self.ascend();
        table
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        self.descend();
        if let Some(generics) = &mut body.generics {
            walk_generic_params(self, generics);
        }
        walk_params(self, &mut body.params.items);
        if let Some(vararg) = &mut body.vararg
            && let Some(vararg_type) = &mut vararg.type_annotation
        {
            *vararg_type = self.transform_type(vararg_type.take());
        }
        if let Some(return_type) = &mut body.return_type {
            *return_type = self.transform_type(return_type.take());
        }
        body.block = self.transform_block(body.block);
        self.ascend();
        body
    }

    fn walk_type(&mut self, type_value: Type) -> Type {
        match type_value {
            Type::Named(named) => walk_named_type(self, named),
            // typeof embeds a real expression, so semantic passes must see it.
            Type::Typeof(typeof_type) => walk_typeof_type(self, typeof_type),
            Type::Table(table) => walk_table_type(self, table),
            Type::Function(function_type) => walk_function_type(self, function_type),
            Type::Optional(mut optional) => {
                self.descend();
                optional.type_value = self.transform_type(optional.type_value);
                self.ascend();
                Type::Optional(optional)
            }
            Type::Union(mut union) => {
                self.descend();
                walk_types(self, &mut union.types.items);
                self.ascend();
                Type::Union(union)
            }
            Type::Intersection(mut intersection) => {
                self.descend();
                walk_types(self, &mut intersection.types.items);
                self.ascend();
                Type::Intersection(intersection)
            }
            Type::Parenthesized(mut parenthesized) => {
                self.descend();
                parenthesized.type_value = self.transform_type(parenthesized.type_value);
                self.ascend();
                Type::Parenthesized(parenthesized)
            }
            Type::Pack(mut pack) => {
                self.descend();
                walk_types(self, &mut pack.types.items);
                self.ascend();
                Type::Pack(pack)
            }
            Type::Variadic(mut variadic) => {
                self.descend();
                variadic.type_value = self.transform_type(variadic.type_value);
                self.ascend();
                Type::Variadic(variadic)
            }
            leaf @ (Type::Name { .. }
            | Type::Singleton(_)
            | Type::GenericPack(_)
            | Type::Error(_)) => leaf,
        }
    }

    fn walk_type_args(&mut self, mut type_args: TypeArgs) -> TypeArgs {
        walk_types(self, &mut type_args.args.items);
        type_args
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

    fn walk_punctuated_vars(&mut self, vars: Punctuated<Var>) -> Punctuated<Var> {
        vars.map_items(|var| self.transform_var(var))
    }

    fn walk_punctuated_params(
        &mut self,
        mut params: Punctuated<Parameter>,
    ) -> Punctuated<Parameter> {
        walk_params(self, &mut params.items);
        params
    }

    fn walk_punctuated_types(&mut self, mut types: Punctuated<Type>) -> Punctuated<Type> {
        walk_types(self, &mut types.items);
        types
    }
}

fn walk_exprs<T>(transform: &mut T, exprs: &mut [Expression])
where
    T: Transform + ?Sized,
{
    for expr in exprs {
        *expr = transform.transform_expr(expr.take());
    }
}

fn walk_types<T>(transform: &mut T, types: &mut [Type])
where
    T: Transform + ?Sized,
{
    for type_value in types {
        *type_value = transform.transform_type(type_value.take());
    }
}

fn walk_params<T>(transform: &mut T, params: &mut [Parameter])
where
    T: Transform + ?Sized,
{
    for param in params {
        if let Some(param_type) = &mut param.type_annotation {
            *param_type = transform.transform_type(param_type.take());
        }
    }
}

fn walk_names<T>(transform: &mut T, names: &mut [AttributedName])
where
    T: Transform + ?Sized,
{
    for name in names {
        if let Some(name_type) = &mut name.type_annotation {
            *name_type = transform.transform_type(name_type.take());
        }
    }
}

fn walk_generic_params<T>(transform: &mut T, generics: &mut GenericTypeList)
where
    T: Transform + ?Sized,
{
    for param in &mut generics.params.items {
        if let Some(default) = &mut param.default {
            *default = transform.transform_type(default.take());
        }
    }
}

fn walk_boxed_call<T>(transform: &mut T, mut call: Box<FunctionCall>) -> Box<FunctionCall>
where
    T: Transform + ?Sized,
{
    *call = transform.transform_function_call(*call);
    call
}

fn walk_boxed_table<T>(transform: &mut T, mut table: Box<TableConstructor>) -> Box<TableConstructor>
where
    T: Transform + ?Sized,
{
    *table = transform.transform_table_constructor(*table);
    table
}

fn walk_assignment<T>(transform: &mut T, mut assignment: Box<Assignment>) -> Statement
where
    T: Transform + ?Sized,
{
    assignment.targets = transform.walk_punctuated_vars(assignment.targets);
    walk_exprs(transform, &mut assignment.values.items);
    Statement::Assignment(assignment)
}

fn walk_do_block<T>(transform: &mut T, mut do_block: Box<DoBlock>) -> Statement
where
    T: Transform + ?Sized,
{
    do_block.block = transform.transform_block(do_block.block);
    Statement::DoBlock(do_block)
}

fn walk_while_loop<T>(transform: &mut T, mut while_loop: Box<WhileLoop>) -> Statement
where
    T: Transform + ?Sized,
{
    while_loop.condition = transform.transform_expr(while_loop.condition);
    while_loop.block = transform.transform_block(while_loop.block);
    Statement::WhileLoop(while_loop)
}

fn walk_repeat_loop<T>(transform: &mut T, mut repeat_loop: Box<RepeatLoop>) -> Statement
where
    T: Transform + ?Sized,
{
    repeat_loop.block = transform.transform_block(repeat_loop.block);
    repeat_loop.condition = transform.transform_expr(repeat_loop.condition);
    Statement::RepeatLoop(repeat_loop)
}

fn walk_numeric_for<T>(transform: &mut T, mut numeric_for: Box<NumericFor>) -> Statement
where
    T: Transform + ?Sized,
{
    if let Some(loop_variable_type) = &mut numeric_for.type_annotation {
        *loop_variable_type = transform.transform_type(loop_variable_type.take());
    }
    numeric_for.start = transform.transform_expr(numeric_for.start);
    numeric_for.limit = transform.transform_expr(numeric_for.limit);
    if let Some(step) = &mut numeric_for.step {
        *step = transform.transform_expr(step.take());
    }
    numeric_for.block = transform.transform_block(numeric_for.block);
    Statement::NumericFor(numeric_for)
}

fn walk_generic_for<T>(transform: &mut T, mut generic_for: Box<GenericFor>) -> Statement
where
    T: Transform + ?Sized,
{
    walk_params(transform, &mut generic_for.names.items);
    walk_exprs(transform, &mut generic_for.exprs.items);
    generic_for.block = transform.transform_block(generic_for.block);
    Statement::GenericFor(generic_for)
}

fn walk_function_decl<T>(transform: &mut T, mut function_decl: Box<FunctionDecl>) -> Statement
where
    T: Transform + ?Sized,
{
    function_decl.body = transform.transform_function_body(function_decl.body);
    Statement::FunctionDecl(function_decl)
}

fn walk_local_function<T>(transform: &mut T, mut local_function: Box<LocalFunction>) -> Statement
where
    T: Transform + ?Sized,
{
    local_function.body = transform.transform_function_body(local_function.body);
    Statement::LocalFunction(local_function)
}

// Lua 5.5
fn walk_global_function<T>(transform: &mut T, mut global_function: Box<GlobalFunction>) -> Statement
where
    T: Transform + ?Sized,
{
    global_function.body = transform.transform_function_body(global_function.body);
    Statement::GlobalFunction(global_function)
}

fn walk_local_assignment<T>(
    transform: &mut T,
    mut local_assignment: Box<LocalAssignment>,
) -> Statement
where
    T: Transform + ?Sized,
{
    walk_names(transform, &mut local_assignment.names.items);
    if let Some(exprs) = &mut local_assignment.exprs {
        walk_exprs(transform, &mut exprs.items);
    }
    Statement::LocalAssignment(local_assignment)
}

// Lua 5.5
fn walk_global_declaration<T>(
    transform: &mut T,
    mut global_declaration: Box<GlobalDeclaration>,
) -> Statement
where
    T: Transform + ?Sized,
{
    walk_names(transform, &mut global_declaration.names.items);
    if let Some(exprs) = &mut global_declaration.exprs {
        walk_exprs(transform, &mut exprs.items);
    }
    Statement::GlobalDeclaration(global_declaration)
}

// Luau
fn walk_compound_assignment<T>(
    transform: &mut T,
    mut compound: Box<CompoundAssignment>,
) -> Statement
where
    T: Transform + ?Sized,
{
    compound.var = transform.transform_var(compound.var);
    compound.expr = transform.transform_expr(compound.expr);
    Statement::CompoundAssignment(compound)
}

// Luau
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
            TypeDeclarationValue::Alias(transform.transform_type(alias_type))
        }
        TypeDeclarationValue::TypeFunction(mut body) => {
            *body = transform.transform_function_body(*body);
            TypeDeclarationValue::TypeFunction(body)
        }
    };
    Statement::TypeDeclaration(type_declaration)
}

fn walk_function_def<T>(transform: &mut T, mut function_def: Box<FunctionDef>) -> Expression
where
    T: Transform + ?Sized,
{
    function_def.body = transform.transform_function_body(function_def.body);
    Expression::FunctionDef(function_def)
}

fn walk_parenthesized<T>(
    transform: &mut T,
    mut parenthesized: Box<ParenthesizedExpression>,
) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    parenthesized.expr = transform.transform_expr(parenthesized.expr);
    transform.ascend();
    Expression::Parenthesized(parenthesized)
}

fn walk_binary_op<T>(transform: &mut T, mut binary_op: Box<BinaryOp>) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    // An operand that regroups when printed is walked at the depth it ends
    // up at.
    let (lhs_levels, rhs_levels) = regrouped_levels(&binary_op);
    for _ in 0..lhs_levels {
        transform.descend();
    }
    binary_op.lhs = transform.transform_expr(binary_op.lhs);
    for _ in 0..lhs_levels {
        transform.ascend();
    }
    for _ in 0..rhs_levels {
        transform.descend();
    }
    binary_op.rhs = transform.transform_expr(binary_op.rhs);
    for _ in 0..rhs_levels {
        transform.ascend();
    }
    transform.ascend();
    Expression::BinaryOp(binary_op)
}

fn walk_unary_op<T>(transform: &mut T, mut unary_op: Box<UnaryOp>) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    unary_op.operand = transform.transform_expr(unary_op.operand);
    transform.ascend();
    Expression::UnaryOp(unary_op)
}

// Luau
fn walk_if_expr<T>(transform: &mut T, mut if_expr: Box<IfExpression>) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    if_expr.condition = transform.transform_expr(if_expr.condition);
    if_expr.then_expr = transform.transform_expr(if_expr.then_expr);
    for clause in &mut if_expr.elseif_clauses {
        clause.condition = transform.transform_expr(clause.condition.take());
        clause.expr = transform.transform_expr(clause.expr.take());
    }
    if_expr.else_expr = transform.transform_expr(if_expr.else_expr);
    transform.ascend();
    Expression::IfExpression(if_expr)
}

// Luau
fn walk_interpolated_string<T>(
    transform: &mut T,
    mut interpolated_string: Box<InterpolatedString>,
) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    for segment in &mut interpolated_string.segments {
        if let Some(interpolated) = &mut segment.expr {
            *interpolated = transform.transform_expr(interpolated.take());
        }
    }
    transform.ascend();
    Expression::InterpolatedString(interpolated_string)
}

// Luau
fn walk_type_cast<T>(transform: &mut T, mut cast: Box<TypeCast>) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    cast.expr = transform.transform_expr(cast.expr);
    cast.type_annotation = transform.transform_type(cast.type_annotation);
    transform.ascend();
    Expression::TypeCast(cast)
}

// Luau
fn walk_type_instantiation<T>(
    transform: &mut T,
    mut instantiation: Box<TypeInstantiation>,
) -> Expression
where
    T: Transform + ?Sized,
{
    transform.descend();
    instantiation.expr = transform.transform_expr(instantiation.expr);
    walk_types(transform, &mut instantiation.type_args.args.items);
    transform.ascend();
    Expression::TypeInstantiation(instantiation)
}

fn walk_named_type<T>(transform: &mut T, mut named: Box<NamedType>) -> Type
where
    T: Transform + ?Sized,
{
    transform.descend();
    if let Some(generics) = &mut named.generics {
        walk_types(transform, &mut generics.args.items);
    }
    transform.ascend();
    Type::Named(named)
}

fn walk_typeof_type<T>(transform: &mut T, mut typeof_type: Box<TypeofType>) -> Type
where
    T: Transform + ?Sized,
{
    transform.descend();
    typeof_type.expr = transform.transform_expr(typeof_type.expr);
    transform.ascend();
    Type::Typeof(typeof_type)
}

fn walk_table_type<T>(transform: &mut T, mut table: Box<TableType>) -> Type
where
    T: Transform + ?Sized,
{
    transform.descend();
    for field in &mut table.fields.items {
        match field {
            TypeField::Named { value, .. } | TypeField::Array { value, .. } => {
                *value = transform.transform_type(value.take());
            }
            TypeField::Indexer { key, value, .. } => {
                *key = transform.transform_type(key.take());
                *value = transform.transform_type(value.take());
            }
        }
    }
    transform.ascend();
    Type::Table(table)
}

fn walk_function_type<T>(transform: &mut T, mut function_type: Box<FunctionType>) -> Type
where
    T: Transform + ?Sized,
{
    transform.descend();
    if let Some(generics) = &mut function_type.generics {
        walk_generic_params(transform, generics);
    }
    for param in &mut function_type.params.items {
        param.type_value = transform.transform_type(param.type_value.take());
    }
    function_type.return_type = transform.transform_type(function_type.return_type);
    transform.ascend();
    Type::Function(function_type)
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
                | Expression::Var(_)
                | Expression::FunctionCall(_)
                | Expression::Parenthesized(_)
                | Expression::TableConstructor(_)
                | Expression::BinaryOp(_)
                | Expression::UnaryOp(_)
                | Expression::IfExpression(_)
                | Expression::InterpolatedString(_)
                | Expression::TypeCast(_)
                | Expression::TypeInstantiation(_)
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

        // local x = 1 + 2
        let synth = Synth::new();
        let sum = synth.binary_op(synth.number("1"), luck_token::BinOp::Add, synth.number("2"));
        let block = synth.block(vec![synth.local(&["x"], vec![sum])], None);
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
            calls: usize,
            args: usize,
            tables: usize,
            bodies: usize,
        }

        impl Transform for HookCounts {
            fn transform_function_call(&mut self, call: FunctionCall) -> FunctionCall {
                self.calls = self.calls.saturating_add(1);
                self.walk_function_call(call)
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
        assert_eq!(counts, HookCounts { calls: 2, args: 2, tables: 2, bodies: 1 });
    }
}
