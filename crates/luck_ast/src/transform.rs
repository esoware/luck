use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

/// Ownership-based AST transform. Override `transform_*` methods to rewrite nodes,
/// call `self.walk_*` inside overrides to recurse into children.
pub trait AstTransform {
    fn transform_block(&mut self, block: Block) -> Block {
        self.walk_block(block)
    }
    fn transform_statement(&mut self, stmt: Statement) -> Statement {
        self.walk_statement(stmt)
    }
    fn transform_expression(&mut self, expr: Expression) -> Expression {
        self.walk_expression(expr)
    }
    fn transform_var(&mut self, var: Var) -> Var {
        self.walk_var(var)
    }
    fn transform_last_statement(&mut self, last: LastStatement) -> LastStatement {
        self.walk_last_statement(last)
    }
    fn transform_type(&mut self, type_value: Type) -> Type {
        self.walk_type(type_value)
    }

    fn walk_block(&mut self, block: Block) -> Block {
        let Block {
            span,
            stmts,
            last_stmt,
        } = block;
        let stmts = stmts
            .into_iter()
            .map(|stmt| self.transform_statement(stmt))
            .collect();
        let last_stmt = last_stmt.map(|last| Box::new(self.transform_last_statement(*last)));
        Block {
            span,
            stmts,
            last_stmt,
        }
    }

    fn walk_statement(&mut self, stmt: Statement) -> Statement {
        match stmt {
            Statement::Assignment(mut assignment) => {
                assignment.targets = self.walk_punctuated_vars(assignment.targets);
                assignment.values = self.walk_punctuated_exprs(assignment.values);
                Statement::Assignment(assignment)
            }
            Statement::FunctionCall(mut call_stmt) => {
                call_stmt.call = self.walk_function_call(call_stmt.call);
                Statement::FunctionCall(call_stmt)
            }
            Statement::DoBlock(mut do_block) => {
                let block = self.transform_block(do_block.block);
                do_block.block = block;
                Statement::DoBlock(do_block)
            }
            Statement::WhileLoop(mut while_loop) => {
                while_loop.condition = self.transform_expression(while_loop.condition);
                while_loop.block = self.transform_block(while_loop.block);
                Statement::WhileLoop(while_loop)
            }
            Statement::RepeatLoop(mut repeat_loop) => {
                repeat_loop.block = self.transform_block(repeat_loop.block);
                repeat_loop.condition = self.transform_expression(repeat_loop.condition);
                Statement::RepeatLoop(repeat_loop)
            }
            Statement::IfStatement(mut if_stmt) => {
                if_stmt.condition = self.transform_expression(if_stmt.condition);
                if_stmt.block = self.transform_block(if_stmt.block);
                if_stmt.elseif_clauses = if_stmt
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.condition = self.transform_expression(clause.condition);
                        clause.block = self.transform_block(clause.block);
                        clause
                    })
                    .collect();
                if_stmt.else_clause = if_stmt.else_clause.map(|mut clause| {
                    clause.block = self.transform_block(clause.block);
                    clause
                });
                Statement::IfStatement(if_stmt)
            }
            Statement::NumericFor(mut numeric_for) => {
                numeric_for.type_annotation = numeric_for
                    .type_annotation
                    .map(|(colon, loop_var_type)| (colon, self.transform_type(loop_var_type)));
                numeric_for.start = self.transform_expression(numeric_for.start);
                numeric_for.limit = self.transform_expression(numeric_for.limit);
                numeric_for.comma2_and_step = numeric_for
                    .comma2_and_step
                    .map(|(comma, step)| (comma, self.transform_expression(step)));
                numeric_for.block = self.transform_block(numeric_for.block);
                Statement::NumericFor(numeric_for)
            }
            Statement::GenericFor(mut generic_for) => {
                generic_for.names = self.walk_punctuated_params(generic_for.names);
                generic_for.exprs = self.walk_punctuated_exprs(generic_for.exprs);
                generic_for.block = self.transform_block(generic_for.block);
                Statement::GenericFor(generic_for)
            }
            Statement::FunctionDecl(mut func_decl) => {
                func_decl.body = self.walk_function_body(func_decl.body);
                Statement::FunctionDecl(func_decl)
            }
            Statement::LocalFunction(mut local_func) => {
                local_func.body = self.walk_function_body(local_func.body);
                Statement::LocalFunction(local_func)
            }
            Statement::LocalAssignment(mut local_assign) => {
                local_assign.names = self.walk_attributed_names(local_assign.names);
                local_assign.equal_and_exprs = local_assign
                    .equal_and_exprs
                    .map(|(equal, exprs)| (equal, self.walk_punctuated_exprs(exprs)));
                Statement::LocalAssignment(local_assign)
            }
            Statement::CompoundAssignment(mut compound) => {
                compound.var = self.transform_var(compound.var);
                compound.expr = self.transform_expression(compound.expr);
                Statement::CompoundAssignment(compound)
            }
            Statement::GlobalFunction(mut global_func) => {
                global_func.body = self.walk_function_body(global_func.body);
                Statement::GlobalFunction(global_func)
            }
            Statement::GlobalDeclaration(mut global_decl) => {
                global_decl.names = self.walk_attributed_names(global_decl.names);
                global_decl.equal_and_exprs = global_decl
                    .equal_and_exprs
                    .map(|(equal, exprs)| (equal, self.walk_punctuated_exprs(exprs)));
                Statement::GlobalDeclaration(global_decl)
            }
            Statement::TypeDeclaration(mut type_decl) => {
                type_decl.generics = type_decl
                    .generics
                    .map(|generics| Box::new(self.walk_generic_type_list(*generics)));
                type_decl.type_value = match type_decl.type_value {
                    TypeDeclarationValue::Alias(alias_type) => {
                        TypeDeclarationValue::Alias(self.transform_type(alias_type))
                    }
                    TypeDeclarationValue::TypeFunction(body) => {
                        TypeDeclarationValue::TypeFunction(Box::new(self.walk_function_body(*body)))
                    }
                };
                Statement::TypeDeclaration(type_decl)
            }
            stmt @ (Statement::EmptyStatement(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalStar(_)
            | Statement::Break(_)
            | Statement::Error(_)) => stmt,
        }
    }

    fn walk_expression(&mut self, expr: Expression) -> Expression {
        match expr {
            expr @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_)) => expr,
            Expression::FunctionDef(mut func_def) => {
                func_def.body = self.walk_function_body(func_def.body);
                Expression::FunctionDef(func_def)
            }
            Expression::Var(var) => {
                let var = self.transform_var(*var);
                Expression::Var(Box::new(var))
            }
            Expression::FunctionCall(call) => {
                let call = self.walk_function_call(*call);
                Expression::FunctionCall(Box::new(call))
            }
            Expression::Parenthesized(mut paren) => {
                paren.expr = self.transform_expression(paren.expr);
                Expression::Parenthesized(paren)
            }
            Expression::TableConstructor(table) => {
                let table = self.walk_table_constructor(*table);
                Expression::TableConstructor(Box::new(table))
            }
            Expression::BinaryOp(mut binop) => {
                binop.left = self.transform_expression(binop.left);
                binop.right = self.transform_expression(binop.right);
                Expression::BinaryOp(binop)
            }
            Expression::UnaryOp(mut unop) => {
                unop.operand = self.transform_expression(unop.operand);
                Expression::UnaryOp(unop)
            }
            Expression::IfExpression(mut if_expr) => {
                if_expr.condition = self.transform_expression(if_expr.condition);
                if_expr.then_expr = self.transform_expression(if_expr.then_expr);
                if_expr.elseif_clauses = if_expr
                    .elseif_clauses
                    .into_iter()
                    .map(|mut clause| {
                        clause.condition = self.transform_expression(clause.condition);
                        clause.expr = self.transform_expression(clause.expr);
                        clause
                    })
                    .collect();
                if_expr.else_expr = self.transform_expression(if_expr.else_expr);
                Expression::IfExpression(if_expr)
            }
            Expression::InterpolatedString(mut interp) => {
                interp.segments = interp
                    .segments
                    .into_iter()
                    .map(|mut segment| {
                        segment.expr = segment.expr.map(|e| self.transform_expression(e));
                        segment
                    })
                    .collect();
                Expression::InterpolatedString(interp)
            }
            Expression::TypeCast(mut cast) => {
                cast.expr = self.transform_expression(cast.expr);
                cast.type_annotation = self.transform_type(cast.type_annotation);
                Expression::TypeCast(cast)
            }
        }
    }

    fn walk_var(&mut self, var: Var) -> Var {
        match var {
            Var::Name(_) => var,
            Var::Index(mut index_expr) => {
                index_expr.prefix = self.transform_expression(index_expr.prefix);
                index_expr.index = self.transform_expression(index_expr.index);
                Var::Index(index_expr)
            }
            Var::FieldAccess(mut field_access) => {
                field_access.prefix = self.transform_expression(field_access.prefix);
                Var::FieldAccess(field_access)
            }
        }
    }

    fn walk_function_body(&mut self, mut body: FunctionBody) -> FunctionBody {
        body.generics = body
            .generics
            .map(|generics| Box::new(self.walk_generic_type_list(*generics)));
        body.params = self.walk_punctuated_params(body.params);
        body.vararg = body.vararg.map(|mut vararg| {
            vararg.type_annotation = vararg
                .type_annotation
                .map(|(colon, vararg_type)| (colon, self.transform_type(vararg_type)));
            vararg
        });
        body.return_type = body
            .return_type
            .map(|(colon, return_type)| (colon, self.transform_type(return_type)));
        body.block = self.transform_block(body.block);
        body
    }

    fn walk_last_statement(&mut self, last: LastStatement) -> LastStatement {
        match last {
            LastStatement::Return(mut ret) => {
                ret.exprs = self.walk_punctuated_exprs(ret.exprs);
                LastStatement::Return(ret)
            }
            last @ (LastStatement::Break(_)
            | LastStatement::Continue(_)
            | LastStatement::Error(_)) => last,
        }
    }

    fn walk_function_call(&mut self, call: FunctionCall) -> FunctionCall {
        let callee = self.transform_expression(call.callee);
        let args = self.walk_function_args(call.args);
        FunctionCall {
            callee,
            args,
            ..call
        }
    }

    fn walk_function_args(&mut self, args: FunctionArgs) -> FunctionArgs {
        match args {
            FunctionArgs::Parenthesized { parens, args } => {
                let args = self.walk_punctuated_exprs(args);
                FunctionArgs::Parenthesized { parens, args }
            }
            FunctionArgs::TableConstructor(table) => {
                let table = self.walk_table_constructor(*table);
                FunctionArgs::TableConstructor(Box::new(table))
            }
            FunctionArgs::StringLiteral(_) => args,
        }
    }

    fn walk_table_constructor(&mut self, mut table: TableConstructor) -> TableConstructor {
        table.fields = table
            .fields
            .into_iter()
            .map(|(field, sep)| {
                let field = match field {
                    Field::Bracketed {
                        span,
                        brackets,
                        key,
                        equal,
                        value,
                    } => Field::Bracketed {
                        span,
                        brackets,
                        key: self.transform_expression(key),
                        equal,
                        value: self.transform_expression(value),
                    },
                    Field::Named {
                        span,
                        name,
                        equal,
                        value,
                    } => Field::Named {
                        span,
                        name,
                        equal,
                        value: self.transform_expression(value),
                    },
                    Field::Positional { span, value } => Field::Positional {
                        span,
                        value: self.transform_expression(value),
                    },
                };
                (field, sep)
            })
            .collect();
        table
    }

    fn walk_punctuated_exprs(&mut self, punct: Punctuated<Expression>) -> Punctuated<Expression> {
        Punctuated {
            items: punct
                .items
                .into_iter()
                .map(|(expr, sep)| (self.transform_expression(expr), sep))
                .collect(),
        }
    }

    fn walk_punctuated_vars(&mut self, punct: Punctuated<Var>) -> Punctuated<Var> {
        Punctuated {
            items: punct
                .items
                .into_iter()
                .map(|(var, sep)| (self.transform_var(var), sep))
                .collect(),
        }
    }

    fn walk_type(&mut self, type_value: Type) -> Type {
        match type_value {
            Type::Named(mut named) => {
                named.generics = named.generics.map(|mut generics| {
                    generics.args = self.walk_punctuated_types(generics.args);
                    generics
                });
                Type::Named(named)
            }
            // typeof embeds a real expression - semantic passes must see it
            Type::Typeof(mut typeof_type) => {
                typeof_type.expr = self.transform_expression(typeof_type.expr);
                Type::Typeof(typeof_type)
            }
            Type::Table(mut table) => {
                table.fields = table
                    .fields
                    .into_iter()
                    .map(|(field, sep)| {
                        let field = match field {
                            TypeField::Named {
                                span,
                                access,
                                name,
                                colon,
                                value,
                            } => TypeField::Named {
                                span,
                                access,
                                name,
                                colon,
                                value: self.transform_type(value),
                            },
                            TypeField::Indexer {
                                span,
                                access,
                                brackets,
                                key,
                                colon,
                                value,
                            } => TypeField::Indexer {
                                span,
                                access,
                                brackets,
                                key: self.transform_type(key),
                                colon,
                                value: self.transform_type(value),
                            },
                            TypeField::Array { span, value } => TypeField::Array {
                                span,
                                value: self.transform_type(value),
                            },
                        };
                        (field, sep)
                    })
                    .collect();
                Type::Table(table)
            }
            Type::Function(mut function_type) => {
                function_type.generics = function_type
                    .generics
                    .map(|generics| self.walk_generic_type_list(generics));
                function_type.params = Punctuated {
                    items: function_type
                        .params
                        .items
                        .into_iter()
                        .map(|(mut param, sep)| {
                            param.type_value = self.transform_type(param.type_value);
                            (param, sep)
                        })
                        .collect(),
                };
                function_type.return_type = self.transform_type(function_type.return_type);
                Type::Function(function_type)
            }
            Type::Optional(mut optional) => {
                optional.type_value = self.transform_type(optional.type_value);
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
            Type::Parenthesized(mut paren) => {
                paren.type_value = self.transform_type(paren.type_value);
                Type::Parenthesized(paren)
            }
            Type::Pack(mut pack) => {
                pack.types = self.walk_punctuated_types(pack.types);
                Type::Pack(pack)
            }
            Type::Variadic(mut variadic) => {
                variadic.type_value = self.transform_type(variadic.type_value);
                Type::Variadic(variadic)
            }
            type_value @ (Type::Singleton(_) | Type::GenericPack(_) | Type::Error(_)) => type_value,
        }
    }

    fn walk_generic_type_list(&mut self, mut generics: GenericTypeList) -> GenericTypeList {
        generics.params = Punctuated {
            items: generics
                .params
                .items
                .into_iter()
                .map(|(mut param, sep)| {
                    param.default = param
                        .default
                        .map(|(equal, default)| (equal, self.transform_type(default)));
                    (param, sep)
                })
                .collect(),
        };
        generics
    }

    fn walk_attributed_names(
        &mut self,
        names: Punctuated<AttributedName>,
    ) -> Punctuated<AttributedName> {
        Punctuated {
            items: names
                .items
                .into_iter()
                .map(|(mut name, sep)| {
                    name.type_annotation = name
                        .type_annotation
                        .map(|(colon, name_type)| (colon, self.transform_type(name_type)));
                    (name, sep)
                })
                .collect(),
        }
    }

    fn walk_punctuated_params(&mut self, punct: Punctuated<Parameter>) -> Punctuated<Parameter> {
        Punctuated {
            items: punct
                .items
                .into_iter()
                .map(|(mut param, sep)| {
                    param.type_annotation = param
                        .type_annotation
                        .map(|(colon, param_type)| (colon, self.transform_type(param_type)));
                    (param, sep)
                })
                .collect(),
        }
    }

    fn walk_punctuated_types(&mut self, punct: Punctuated<Type>) -> Punctuated<Type> {
        Punctuated {
            items: punct
                .items
                .into_iter()
                .map(|(type_value, sep)| (self.transform_type(type_value), sep))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visitor::Visitor;
    use luck_token::BinOp;
    use luck_token::Span;
    use luck_token::token::{Token, TokenKind};

    fn span() -> Span {
        Span::new(0, 0)
    }

    fn token(kind: TokenKind) -> Token {
        Token::new(kind, span())
    }

    fn num_expr() -> Expression {
        Expression::Number(token(TokenKind::Number("1".into())))
    }

    fn nil_expr() -> Expression {
        Expression::Nil(span())
    }

    fn binop_expr(left: Expression, right: Expression) -> Expression {
        Expression::BinaryOp(Box::new(BinaryOp {
            span: span(),
            left,
            op: BinOp::Add,
            op_span: span(),
            right,
        }))
    }

    fn empty_block() -> Block {
        Block {
            span: span(),
            stmts: Vec::new(),
            last_stmt: None,
        }
    }

    struct NilCounter(usize);
    impl<'ast> Visitor<'ast> for NilCounter {
        fn visit_expression(&mut self, expr: &'ast Expression) {
            if matches!(expr, Expression::Nil(_)) {
                self.0 += 1;
            }
            self.walk_expression(expr);
        }
    }

    struct FalseCounter(usize);
    impl<'ast> Visitor<'ast> for FalseCounter {
        fn visit_expression(&mut self, expr: &'ast Expression) {
            if matches!(expr, Expression::False(_)) {
                self.0 += 1;
            }
            self.walk_expression(expr);
        }
    }

    struct NilToFalse;
    impl AstTransform for NilToFalse {
        fn transform_expression(&mut self, expr: Expression) -> Expression {
            let expr = self.walk_expression(expr);
            match expr {
                Expression::Nil(token) => Expression::False(token),
                other => other,
            }
        }
    }

    #[test]
    fn nil_to_false_in_local_assignment() {
        // local x = nil => local x = false
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalAssignment(Box::new(LocalAssignment {
                span: span(),
                local_token: span(),
                is_const: false,
                names: Punctuated::from_item(AttributedName {
                    name: token(TokenKind::Identifier("x".into())),
                    type_annotation: None,
                    attrib: None,
                }),
                equal_and_exprs: Some((span(), Punctuated::from_item(nil_expr()))),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
        let mut false_count = FalseCounter(0);
        false_count.visit_block(&block);
        assert_eq!(false_count.0, 1);
    }

    #[test]
    fn nil_to_false_in_table() {
        // {nil, nil} => {false, false}
        let table = Expression::TableConstructor(Box::new(TableConstructor {
            span: span(),
            braces: ContainedSpan {
                open: span(),
                close: span(),
            },
            fields: vec![
                (
                    Field::Positional {
                        span: span(),
                        value: nil_expr(),
                    },
                    Some(span()),
                ),
                (
                    Field::Positional {
                        span: span(),
                        value: nil_expr(),
                    },
                    None,
                ),
            ],
        }));
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalAssignment(Box::new(LocalAssignment {
                span: span(),
                local_token: span(),
                is_const: false,
                names: Punctuated::from_item(AttributedName {
                    name: token(TokenKind::Identifier("t".into())),
                    type_annotation: None,
                    attrib: None,
                }),
                equal_and_exprs: Some((span(), Punctuated::from_item(table))),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
        let mut false_count = FalseCounter(0);
        false_count.visit_block(&block);
        assert_eq!(false_count.0, 2);
    }

    #[test]
    fn nil_to_false_in_function_return() {
        // local function f() return nil end => return false
        let body = FunctionBody {
            span: span(),
            generics: None,
            params_parens: ContainedSpan {
                open: span(),
                close: span(),
            },
            params: Punctuated::empty(),
            vararg: None,
            return_type: None,
            block: Block {
                span: span(),
                stmts: Vec::new(),
                last_stmt: Some(Box::new(LastStatement::Return(Box::new(ReturnStatement {
                    span: span(),
                    return_token: span(),
                    exprs: Punctuated::from_item(nil_expr()),
                    semicolon: None,
                })))),
            },
            end_token: span(),
        };
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalFunction(Box::new(LocalFunction {
                span: span(),
                attributes: Vec::new(),
                local_token: span(),
                is_const: false,
                function_token: span(),
                name: token(TokenKind::Identifier("f".into())),
                body,
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
        let mut false_count = FalseCounter(0);
        false_count.visit_block(&block);
        assert_eq!(false_count.0, 1);
    }

    #[test]
    fn nil_to_false_in_if_statement() {
        // if nil then x = nil else y = nil end => all nils become false
        let block = Block {
            span: span(),
            stmts: vec![Statement::IfStatement(Box::new(IfStatement {
                span: span(),
                if_token: span(),
                condition: nil_expr(),
                then_token: span(),
                block: Block {
                    span: span(),
                    stmts: vec![Statement::Assignment(Box::new(Assignment {
                        span: span(),
                        targets: Punctuated::from_item(Var::Name(token(TokenKind::Identifier(
                            "x".into(),
                        )))),
                        equal: span(),
                        values: Punctuated::from_item(nil_expr()),
                    }))],
                    last_stmt: None,
                },
                elseif_clauses: Vec::new(),
                else_clause: Some(ElseClause {
                    span: span(),
                    else_token: span(),
                    block: Block {
                        span: span(),
                        stmts: vec![Statement::Assignment(Box::new(Assignment {
                            span: span(),
                            targets: Punctuated::from_item(Var::Name(token(
                                TokenKind::Identifier("y".into()),
                            ))),
                            equal: span(),
                            values: Punctuated::from_item(nil_expr()),
                        }))],
                        last_stmt: None,
                    },
                }),
                end_token: span(),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
        let mut false_count = FalseCounter(0);
        false_count.visit_block(&block);
        assert_eq!(false_count.0, 3);
    }

    #[test]
    fn identity_transform_preserves_structure() {
        struct Identity;
        impl AstTransform for Identity {}

        // local x = 1 + 2
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalAssignment(Box::new(LocalAssignment {
                span: span(),
                local_token: span(),
                is_const: false,
                names: Punctuated::from_item(AttributedName {
                    name: token(TokenKind::Identifier("x".into())),
                    type_annotation: None,
                    attrib: None,
                }),
                equal_and_exprs: Some((
                    span(),
                    Punctuated::from_item(binop_expr(num_expr(), num_expr())),
                )),
            }))],
            last_stmt: None,
        };
        let original = block.clone();
        let transformed = Identity.transform_block(block);
        assert_eq!(original, transformed);
    }

    #[test]
    fn transform_while_loop_condition() {
        // while nil do end => while false do end
        let block = Block {
            span: span(),
            stmts: vec![Statement::WhileLoop(Box::new(WhileLoop {
                span: span(),
                while_token: span(),
                condition: nil_expr(),
                do_token: span(),
                block: empty_block(),
                end_token: span(),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
    }

    #[test]
    fn transform_repeat_loop_condition() {
        // repeat until nil => repeat until false
        let block = Block {
            span: span(),
            stmts: vec![Statement::RepeatLoop(Box::new(RepeatLoop {
                span: span(),
                repeat_token: span(),
                block: empty_block(),
                until_token: span(),
                condition: nil_expr(),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
    }

    #[test]
    fn transform_numeric_for_step() {
        // for i = nil, nil, nil do end => all become false
        let block = Block {
            span: span(),
            stmts: vec![Statement::NumericFor(Box::new(NumericFor {
                span: span(),
                for_token: span(),
                name: token(TokenKind::Identifier("i".into())),
                type_annotation: None,
                equal: span(),
                start: nil_expr(),
                comma1: span(),
                limit: nil_expr(),
                comma2_and_step: Some((span(), nil_expr())),
                do_token: span(),
                block: empty_block(),
                end_token: span(),
            }))],
            last_stmt: None,
        };
        let block = NilToFalse.transform_block(block);
        let mut nil_count = NilCounter(0);
        nil_count.visit_block(&block);
        assert_eq!(nil_count.0, 0);
        let mut false_count = FalseCounter(0);
        false_count.visit_block(&block);
        assert_eq!(false_count.0, 3);
    }
}
