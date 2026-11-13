use core::mem;

use luck_token::Span;

use crate::expr::{Expression, Field, FunctionArgs, FunctionCall, TableConstructor, Var};
use crate::function::{FunctionBody, Parameter};
use crate::punctuated::Punctuated;
use crate::stmt::{
    AttributedName, Block, IfStatement, LastStatement, Statement, TypeDeclarationValue,
};
use crate::types::{GenericTypeList, Type, TypeArgs, TypeField};

/// Each `transform_*` hook defaults to its `walk_*`, which rebuilds the node
/// from its transformed children.
///
/// Override the hooks you need and call `self.walk_*` from them to recurse. The `walk_*` methods
/// are the default recursion, so overriding one instead of its hook would leave nothing to fall
/// back on.
pub trait Transform {
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

    fn walk_block(&mut self, block: Block) -> Block {
        Block {
            span: block.span,
            stmts: block.stmts.into_iter().map(|stmt| self.transform_stmt(stmt)).collect(),
            last_stmt: block.last_stmt.map(|mut last| {
                *last = self.transform_last_stmt(*last);
                last
            }),
        }
    }

    fn walk_stmt(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Assignment(mut assignment) => {
                assignment.targets = self.walk_punctuated_vars(assignment.targets);
                assignment.values = self.walk_punctuated_exprs(assignment.values);
                Statement::Assignment(assignment)
            }
            Statement::FunctionCall(mut call) => {
                *call = self.transform_function_call(*call);
                Statement::FunctionCall(call)
            }
            Statement::DoBlock(mut do_block) => {
                do_block.block = self.transform_block(do_block.block);
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = self.transform_expr(while_loop.condition);
                while_loop.block = self.transform_block(while_loop.block);
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                repeat_loop.block = self.transform_block(repeat_loop.block);
                repeat_loop.condition = self.transform_expr(repeat_loop.condition);
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(if_stmt) => Statement::IfStatement(self.walk_if_stmt(if_stmt)),
            Statement::NumericFor(mut numeric_for) => {
                numeric_for.type_annotation = numeric_for
                    .type_annotation
                    .map(|loop_variable_type| self.transform_type(loop_variable_type));
                numeric_for.start = self.transform_expr(numeric_for.start);
                numeric_for.limit = self.transform_expr(numeric_for.limit);
                numeric_for.step = numeric_for.step.map(|step| self.transform_expr(step));
                numeric_for.block = self.transform_block(numeric_for.block);
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                generic_for.names = self.walk_punctuated_params(generic_for.names);
                generic_for.exprs = self.walk_punctuated_exprs(generic_for.exprs);
                generic_for.block = self.transform_block(generic_for.block);
                Statement::GenericFor(generic_for)
            }
            Statement::FunctionDecl(mut function_decl) => {
                function_decl.body = self.transform_function_body(function_decl.body);
                Statement::FunctionDecl(function_decl)
            }
            Statement::LocalFunction(mut local_function) => {
                local_function.body = self.transform_function_body(local_function.body);
                Statement::LocalFunction(local_function)
            }
            Statement::LocalAssignment(mut local_assignment) => {
                local_assignment.names = self.walk_attributed_names(local_assignment.names);
                local_assignment.exprs =
                    local_assignment.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
                Statement::LocalAssignment(local_assignment)
            }
            // Luau
            Statement::CompoundAssignment(mut compound) => {
                compound.var = self.transform_var(compound.var);
                compound.expr = self.transform_expr(compound.expr);
                Statement::CompoundAssignment(compound)
            }
            // Lua 5.5
            Statement::GlobalFunction(mut global_function) => {
                global_function.body = self.transform_function_body(global_function.body);
                Statement::GlobalFunction(global_function)
            }
            // Lua 5.5
            Statement::GlobalDeclaration(mut global_declaration) => {
                global_declaration.names = self.walk_attributed_names(global_declaration.names);
                global_declaration.exprs =
                    global_declaration.exprs.map(|exprs| self.walk_punctuated_exprs(exprs));
                Statement::GlobalDeclaration(global_declaration)
            }
            // Luau
            Statement::TypeDeclaration(mut type_declaration) => {
                type_declaration.generics = type_declaration.generics.map(|mut generics| {
                    *generics = self.walk_generic_type_list(*generics);
                    generics
                });
                type_declaration.type_value = match type_declaration.type_value {
                    TypeDeclarationValue::Alias(alias_type) => {
                        TypeDeclarationValue::Alias(self.transform_type(alias_type))
                    }
                    TypeDeclarationValue::TypeFunction(mut body) => {
                        *body = self.transform_function_body(*body);
                        TypeDeclarationValue::TypeFunction(body)
                    }
                };
                Statement::TypeDeclaration(type_declaration)
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
        if_stmt.elseif_clauses = if_stmt
            .elseif_clauses
            .into_iter()
            .map(|mut clause| {
                clause.condition = self.transform_expr(clause.condition);
                clause.block = self.transform_block(clause.block);
                clause
            })
            .collect();
        if_stmt.else_clause = if_stmt.else_clause.map(|mut clause| {
            clause.block = self.transform_block(clause.block);
            clause
        });
        if_stmt
    }

    fn walk_last_stmt(&mut self, last: LastStatement) -> LastStatement {
        match last {
            LastStatement::Return(mut return_stmt) => {
                return_stmt.exprs = self.walk_punctuated_exprs(return_stmt.exprs);
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
            Expression::FunctionDef(mut function_def) => {
                function_def.body = self.transform_function_body(function_def.body);
                Expression::FunctionDef(function_def)
            }
            Expression::Var(var) => Expression::Var(self.transform_var(var)),
            Expression::FunctionCall(mut call) => {
                *call = self.transform_function_call(*call);
                Expression::FunctionCall(call)
            }
            Expression::Parenthesized(mut parenthesized) => {
                parenthesized.expr = self.transform_expr(parenthesized.expr.take());
                Expression::Parenthesized(parenthesized)
            }
            Expression::TableConstructor(mut table) => {
                *table = self.transform_table_constructor(*table);
                Expression::TableConstructor(table)
            }
            Expression::BinaryOp(mut binary_op) => {
                binary_op.lhs = self.transform_expr(binary_op.lhs.take());
                binary_op.rhs = self.transform_expr(binary_op.rhs.take());
                Expression::BinaryOp(binary_op)
            }
            Expression::UnaryOp(mut unary_op) => {
                unary_op.operand = self.transform_expr(unary_op.operand.take());
                Expression::UnaryOp(unary_op)
            }
            // Luau
            Expression::IfExpression(mut if_expr) => {
                if_expr.condition = self.transform_expr(if_expr.condition);
                if_expr.then_expr = self.transform_expr(if_expr.then_expr);
                if_expr.elseif_clauses = if_expr
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.condition = self.transform_expr(clause.condition);
                        clause.expr = self.transform_expr(clause.expr);
                        clause
                    })
                    .collect();
                if_expr.else_expr = self.transform_expr(if_expr.else_expr);
                Expression::IfExpression(if_expr)
            }
            // Luau
            Expression::InterpolatedString(mut interpolated_string) => {
                interpolated_string.segments = interpolated_string
                    .segments
                    .into_iter()
                    .map(|mut segment| {
                        segment.expr =
                            segment.expr.map(|interpolated| self.transform_expr(interpolated));
                        segment
                    })
                    .collect();
                Expression::InterpolatedString(interpolated_string)
            }
            // Luau
            Expression::TypeCast(mut cast) => {
                cast.expr = self.transform_expr(cast.expr.take());
                cast.type_annotation = self.transform_type(cast.type_annotation.take());
                Expression::TypeCast(cast)
            }
            // Luau
            Expression::TypeInstantiation(mut instantiation) => {
                instantiation.expr = self.transform_expr(instantiation.expr.take());
                let type_args = mem::replace(
                    &mut instantiation.type_args,
                    TypeArgs { span: Span::default(), args: Punctuated::empty() },
                );
                instantiation.type_args = self.walk_type_args(type_args);
                Expression::TypeInstantiation(instantiation)
            }
        }
    }

    fn walk_var(&mut self, var: Var) -> Var {
        match var {
            Var::Name(_) => var,
            Var::Index(mut index_expr) => {
                index_expr.prefix = self.transform_expr(index_expr.prefix.take());
                index_expr.index = self.transform_expr(index_expr.index.take());
                Var::Index(index_expr)
            }
            Var::FieldAccess(mut field_access) => {
                field_access.prefix = self.transform_expr(field_access.prefix.take());
                Var::FieldAccess(field_access)
            }
        }
    }

    /// Rewrites the callee, then any explicit type arguments, then the
    /// arguments, in source order as the `Visitor` walk reaches them.
    fn walk_function_call(&mut self, mut call: FunctionCall) -> FunctionCall {
        call.callee = self.transform_expr(call.callee.take());
        call.explicit_type_args = call.explicit_type_args.take().map(|mut type_args| {
            *type_args = self.walk_type_args(*type_args);
            type_args
        });
        call.args = self.transform_function_args(call.args.take());
        call
    }

    fn walk_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        match args {
            FunctionArgs::Parenthesized { span, args: exprs } => {
                FunctionArgs::Parenthesized { span, args: self.walk_punctuated_exprs(exprs) }
            }
            FunctionArgs::TableConstructor(mut table) => {
                *table = self.transform_table_constructor(*table);
                FunctionArgs::TableConstructor(table)
            }
            FunctionArgs::StringLiteral(_) => args,
        }
    }

    fn walk_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        table.fields = table.fields.map_items(|field| match field {
            Field::Bracketed { span, key, value } => Field::Bracketed {
                span,
                key: self.transform_expr(key),
                value: self.transform_expr(value),
            },
            Field::Named { span, name, value } => {
                Field::Named { span, name, value: self.transform_expr(value) }
            }
            Field::Positional { span, value } => {
                Field::Positional { span, value: self.transform_expr(value) }
            }
        });
        table
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        body.generics = body.generics.map(|mut generics| {
            *generics = self.walk_generic_type_list(*generics);
            generics
        });
        body.params = self.walk_punctuated_params(body.params);
        body.vararg = body.vararg.map(|mut vararg| {
            vararg.type_annotation =
                vararg.type_annotation.map(|vararg_type| self.transform_type(vararg_type));
            vararg
        });
        body.return_type = body.return_type.map(|return_type| self.transform_type(return_type));
        body.block = self.transform_block(body.block);
        body
    }

    fn walk_type(&mut self, type_value: Type) -> Type {
        match type_value {
            Type::Named(mut named) => {
                named.generics = named.generics.map(|generics| self.walk_type_args(generics));
                Type::Named(named)
            }
            // typeof embeds a real expression, so semantic passes must see it.
            Type::Typeof(mut typeof_type) => {
                typeof_type.expr = self.transform_expr(typeof_type.expr);
                Type::Typeof(typeof_type)
            }
            Type::Table(mut table) => {
                table.fields = table.fields.map_items(|field| match field {
                    TypeField::Named { span, access, name, value } => {
                        TypeField::Named { span, access, name, value: self.transform_type(value) }
                    }
                    TypeField::Indexer { span, access, key, value } => TypeField::Indexer {
                        span,
                        access,
                        key: self.transform_type(key),
                        value: self.transform_type(value),
                    },
                    TypeField::Array { span, access, value } => {
                        TypeField::Array { span, access, value: self.transform_type(value) }
                    }
                });
                Type::Table(table)
            }
            Type::Function(mut function_type) => {
                function_type.generics =
                    function_type.generics.map(|generics| self.walk_generic_type_list(generics));
                function_type.params = function_type.params.map_items(|mut param| {
                    param.type_value = self.transform_type(param.type_value);
                    param
                });
                function_type.return_type = self.transform_type(function_type.return_type);
                Type::Function(function_type)
            }
            Type::Optional(mut optional) => {
                optional.type_value = self.transform_type(optional.type_value.take());
                Type::Optional(optional)
            }
            Type::Union(mut union) => {
                union.types = self.walk_punctuated_types(union.types);
                Type::Union(union)
            }
            Type::Intersection(mut intersection) => {
                intersection.types = self.walk_punctuated_types(intersection.types);
                Type::Intersection(intersection)
            }
            Type::Parenthesized(mut parenthesized) => {
                parenthesized.type_value = self.transform_type(parenthesized.type_value);
                Type::Parenthesized(parenthesized)
            }
            Type::Pack(mut pack) => {
                pack.types = self.walk_punctuated_types(pack.types);
                Type::Pack(pack)
            }
            Type::Variadic(mut variadic) => {
                variadic.type_value = self.transform_type(variadic.type_value);
                Type::Variadic(variadic)
            }
            leaf @ (Type::Name { .. }
            | Type::Singleton(_)
            | Type::GenericPack(_)
            | Type::Error(_)) => leaf,
        }
    }

    fn walk_type_args(&mut self, mut type_args: TypeArgs) -> TypeArgs {
        type_args.args = self.walk_punctuated_types(type_args.args);
        type_args
    }

    fn walk_generic_type_list(&mut self, mut generics: GenericTypeList) -> GenericTypeList {
        generics.params = generics.params.map_items(|mut param| {
            param.default = param.default.map(|default| self.transform_type(default));
            param
        });
        generics
    }

    fn walk_attributed_names(
        &mut self,
        names: Punctuated<AttributedName>,
    ) -> Punctuated<AttributedName> {
        names.map_items(|mut name| {
            name.type_annotation =
                name.type_annotation.map(|name_type| self.transform_type(name_type));
            name
        })
    }

    fn walk_punctuated_exprs(&mut self, exprs: Punctuated<Expression>) -> Punctuated<Expression> {
        exprs.map_items(|expr| self.transform_expr(expr))
    }

    fn walk_punctuated_vars(&mut self, vars: Punctuated<Var>) -> Punctuated<Var> {
        vars.map_items(|var| self.transform_var(var))
    }

    fn walk_punctuated_params(&mut self, params: Punctuated<Parameter>) -> Punctuated<Parameter> {
        params.map_items(|mut param| {
            param.type_annotation =
                param.type_annotation.map(|param_type| self.transform_type(param_type));
            param
        })
    }

    fn walk_punctuated_types(&mut self, types: Punctuated<Type>) -> Punctuated<Type> {
        types.map_items(|type_value| self.transform_type(type_value))
    }
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
