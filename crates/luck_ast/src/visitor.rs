use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

/// Read-only AST traversal. Override `visit_*` methods to inspect nodes,
/// call `self.walk_*` inside overrides to continue recursion.
pub trait Visitor<'ast> {
    fn visit_block(&mut self, block: &'ast Block) {
        self.walk_block(block);
    }
    fn visit_statement(&mut self, stmt: &'ast Statement) {
        self.walk_statement(stmt);
    }
    fn visit_expression(&mut self, expr: &'ast Expression) {
        self.walk_expression(expr);
    }
    fn visit_var(&mut self, var: &'ast Var) {
        self.walk_var(var);
    }
    fn visit_function_body(&mut self, body: &'ast FunctionBody) {
        self.walk_function_body(body);
    }
    fn visit_last_statement(&mut self, last: &'ast LastStatement) {
        self.walk_last_statement(last);
    }
    fn visit_type(&mut self, type_value: &'ast Type) {
        self.walk_type(type_value);
    }

    fn walk_block(&mut self, block: &'ast Block) {
        for stmt in &block.stmts {
            self.visit_statement(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.visit_last_statement(last);
        }
    }

    fn walk_statement(&mut self, stmt: &'ast Statement) {
        match stmt {
            Statement::Assignment(assignment) => {
                for var in assignment.targets.iter() {
                    self.visit_var(var);
                }
                for expr in assignment.values.iter() {
                    self.visit_expression(expr);
                }
            }
            Statement::FunctionCall(call_stmt) => {
                self.walk_function_call(&call_stmt.call);
            }
            Statement::DoBlock(do_block) => {
                self.visit_block(&do_block.block);
            }
            Statement::WhileLoop(while_loop) => {
                self.visit_expression(&while_loop.condition);
                self.visit_block(&while_loop.block);
            }
            Statement::RepeatLoop(repeat_loop) => {
                self.visit_block(&repeat_loop.block);
                self.visit_expression(&repeat_loop.condition);
            }
            Statement::IfStatement(if_stmt) => {
                self.visit_expression(&if_stmt.condition);
                self.visit_block(&if_stmt.block);
                for clause in &if_stmt.elseif_clauses {
                    self.visit_expression(&clause.condition);
                    self.visit_block(&clause.block);
                }
                if let Some(else_clause) = &if_stmt.else_clause {
                    self.visit_block(&else_clause.block);
                }
            }
            Statement::NumericFor(numeric_for) => {
                if let Some(loop_var_type) = &numeric_for.type_annotation {
                    self.visit_type(loop_var_type);
                }
                self.visit_expression(&numeric_for.start);
                self.visit_expression(&numeric_for.limit);
                if let Some(step) = &numeric_for.step {
                    self.visit_expression(step);
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
                    self.visit_expression(expr);
                }
                self.visit_block(&generic_for.block);
            }
            Statement::FunctionDecl(func_decl) => {
                self.visit_function_body(&func_decl.body);
            }
            Statement::LocalFunction(local_func) => {
                self.visit_function_body(&local_func.body);
            }
            Statement::LocalAssignment(local_assign) => {
                self.walk_attributed_names(&local_assign.names);
                if let Some(exprs) = &local_assign.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
            }
            Statement::CompoundAssignment(compound) => {
                self.visit_var(&compound.var);
                self.visit_expression(&compound.expr);
            }
            Statement::GlobalFunction(global_func) => {
                self.visit_function_body(&global_func.body);
            }
            Statement::GlobalDeclaration(global_decl) => {
                self.walk_attributed_names(&global_decl.names);
                if let Some(exprs) = &global_decl.exprs {
                    for expr in exprs.iter() {
                        self.visit_expression(expr);
                    }
                }
            }
            Statement::TypeDeclaration(type_decl) => {
                if let Some(generics) = &type_decl.generics {
                    self.walk_generic_type_list(generics);
                }
                match &type_decl.type_value {
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

    fn walk_expression(&mut self, expr: &'ast Expression) {
        match expr {
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::Error(_) => {}
            Expression::FunctionDef(func_def) => {
                self.visit_function_body(&func_def.body);
            }
            Expression::Var(var) => {
                self.visit_var(var);
            }
            Expression::FunctionCall(call) => {
                self.walk_function_call(call);
            }
            Expression::Parenthesized(paren) => {
                self.visit_expression(&paren.expr);
            }
            Expression::TableConstructor(table) => {
                self.walk_table_constructor(table);
            }
            Expression::BinaryOp(binop) => {
                self.visit_expression(&binop.left);
                self.visit_expression(&binop.right);
            }
            Expression::UnaryOp(unop) => {
                self.visit_expression(&unop.operand);
            }
            Expression::IfExpression(if_expr) => {
                self.visit_expression(&if_expr.condition);
                self.visit_expression(&if_expr.then_expr);
                for clause in &if_expr.elseif_clauses {
                    self.visit_expression(&clause.condition);
                    self.visit_expression(&clause.expr);
                }
                self.visit_expression(&if_expr.else_expr);
            }
            Expression::InterpolatedString(interp) => {
                for segment in &interp.segments {
                    if let Some(expr) = &segment.expr {
                        self.visit_expression(expr);
                    }
                }
            }
            Expression::TypeCast(cast) => {
                self.visit_expression(&cast.expr);
                self.visit_type(&cast.type_annotation);
            }
        }
    }

    fn walk_var(&mut self, var: &'ast Var) {
        match var {
            Var::Name(_) => {}
            Var::Index(index_expr) => {
                self.visit_expression(&index_expr.prefix);
                self.visit_expression(&index_expr.index);
            }
            Var::FieldAccess(field_access) => {
                self.visit_expression(&field_access.prefix);
            }
        }
    }

    fn walk_function_body(&mut self, body: &'ast FunctionBody) {
        if let Some(generics) = &body.generics {
            self.walk_generic_type_list(generics);
        }
        for param in body.params.iter() {
            if let Some(param_type) = &param.type_annotation {
                self.visit_type(param_type);
            }
        }
        if let Some(vararg) = &body.vararg {
            if let Some(vararg_type) = &vararg.type_annotation {
                self.visit_type(vararg_type);
            }
        }
        if let Some(return_type) = &body.return_type {
            self.visit_type(return_type);
        }
        self.visit_block(&body.block);
    }

    fn walk_last_statement(&mut self, last: &'ast LastStatement) {
        match last {
            LastStatement::Return(ret) => {
                for expr in ret.exprs.iter() {
                    self.visit_expression(expr);
                }
            }
            LastStatement::Break(_) | LastStatement::Continue(_) | LastStatement::Error(_) => {}
        }
    }

    fn walk_function_call(&mut self, call: &'ast FunctionCall) {
        self.visit_expression(&call.callee);
        self.walk_function_args(&call.args);
    }

    fn walk_function_args(&mut self, args: &'ast FunctionArgs) {
        match args {
            FunctionArgs::Parenthesized { args, .. } => {
                for expr in args.iter() {
                    self.visit_expression(expr);
                }
            }
            FunctionArgs::TableConstructor(table) => {
                self.walk_table_constructor(table);
            }
            FunctionArgs::StringLiteral(_) => {}
        }
    }

    fn walk_table_constructor(&mut self, table: &'ast TableConstructor) {
        for field in table.fields.iter() {
            match field {
                Field::Bracketed { key, value, .. } => {
                    self.visit_expression(key);
                    self.visit_expression(value);
                }
                Field::Named { value, .. } => {
                    self.visit_expression(value);
                }
                Field::Positional { value, .. } => {
                    self.visit_expression(value);
                }
            }
        }
    }

    fn walk_type(&mut self, type_value: &'ast Type) {
        match type_value {
            Type::Named(named) => {
                if let Some(generics) = &named.generics {
                    for arg in generics.args.iter() {
                        self.visit_type(arg);
                    }
                }
            }
            // typeof embeds a real expression - semantic passes must see it
            Type::Typeof(typeof_type) => {
                self.visit_expression(&typeof_type.expr);
            }
            Type::Table(table) => {
                for field in table.fields.iter() {
                    match field {
                        TypeField::Named { value, .. } => self.visit_type(value),
                        TypeField::Indexer { key, value, .. } => {
                            self.visit_type(key);
                            self.visit_type(value);
                        }
                        TypeField::Array { value, .. } => self.visit_type(value),
                    }
                }
            }
            Type::Function(function_type) => {
                if let Some(generics) = &function_type.generics {
                    self.walk_generic_type_list(generics);
                }
                for param in function_type.params.iter() {
                    self.visit_type(&param.type_value);
                }
                self.visit_type(&function_type.return_type);
            }
            Type::Optional(optional) => self.visit_type(&optional.type_value),
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
            Type::Parenthesized(paren) => self.visit_type(&paren.type_value),
            Type::Pack(pack) => {
                for item in pack.types.iter() {
                    self.visit_type(item);
                }
            }
            Type::Variadic(variadic) => self.visit_type(&variadic.type_value),
            Type::Singleton(_) | Type::GenericPack(_) | Type::Error(_) => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
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
        Expression::Number(Literal {
            text: "1".into(),
            span: span(),
        })
    }

    fn name_expr(name: &str) -> Expression {
        Expression::Var(Box::new(Var::Name(token(TokenKind::Identifier(
            name.into(),
        )))))
    }

    fn nil_expr() -> Expression {
        Expression::Nil(span())
    }

    fn binop_expr(left: Expression, right: Expression) -> Expression {
        Expression::BinaryOp(Box::new(BinaryOp {
            span: span(),
            left,
            op: BinOp::Add,
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

    struct ExprCounter(usize);
    impl<'ast> Visitor<'ast> for ExprCounter {
        fn visit_expression(&mut self, expr: &'ast Expression) {
            self.0 += 1;
            self.walk_expression(expr);
        }
    }

    struct StmtCounter(usize);
    impl<'ast> Visitor<'ast> for StmtCounter {
        fn visit_statement(&mut self, stmt: &'ast Statement) {
            self.0 += 1;
            self.walk_statement(stmt);
        }
    }

    #[test]
    fn count_expressions_in_binary_op() {
        // local x = 1 + 2 => 3 expressions (1, 2, 1+2)
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalAssignment(Box::new(LocalAssignment {
                span: span(),
                is_const: false,
                names: Punctuated::from_item(AttributedName {
                    name: token(TokenKind::Identifier("x".into())),
                    type_annotation: None,
                    attrib: None,
                }),
                exprs: Some(Punctuated::from_item(binop_expr(num_expr(), num_expr()))),
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 3);
    }

    #[test]
    fn count_expressions_in_table() {
        // {1, 2, 3} => table constructor + 3 values = 4
        let table = Expression::TableConstructor(Box::new(TableConstructor {
            span: span(),
            fields: Punctuated::from_items(vec![
                Field::Positional {
                    span: span(),
                    value: num_expr(),
                },
                Field::Positional {
                    span: span(),
                    value: num_expr(),
                },
                Field::Positional {
                    span: span(),
                    value: num_expr(),
                },
            ]),
        }));
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalAssignment(Box::new(LocalAssignment {
                span: span(),
                is_const: false,
                names: Punctuated::from_item(AttributedName {
                    name: token(TokenKind::Identifier("t".into())),
                    type_annotation: None,
                    attrib: None,
                }),
                exprs: Some(Punctuated::from_item(table)),
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 4);
    }

    #[test]
    fn count_statements() {
        let block = Block {
            span: span(),
            stmts: vec![
                Statement::LocalAssignment(Box::new(LocalAssignment {
                    span: span(),
                    is_const: false,
                    names: Punctuated::from_item(AttributedName {
                        name: token(TokenKind::Identifier("x".into())),
                        type_annotation: None,
                        attrib: None,
                    }),
                    exprs: Some(Punctuated::from_item(num_expr())),
                })),
                Statement::LocalAssignment(Box::new(LocalAssignment {
                    span: span(),
                    is_const: false,
                    names: Punctuated::from_item(AttributedName {
                        name: token(TokenKind::Identifier("y".into())),
                        type_annotation: None,
                        attrib: None,
                    }),
                    exprs: Some(Punctuated::from_item(num_expr())),
                })),
            ],
            last_stmt: Some(Box::new(LastStatement::Return(Box::new(ReturnStatement {
                span: span(),
                exprs: Punctuated::from_item(name_expr("x")),
            })))),
        };
        let mut counter = StmtCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 2);
    }

    #[test]
    fn visit_function_body() {
        // local function f(a) return a + 1 end => 3 expressions (a, 1, a+1)
        let body = FunctionBody {
            span: span(),
            generics: None,
            params: Punctuated::from_item(Parameter {
                span: span(),
                name: token(TokenKind::Identifier("a".into())),
                type_annotation: None,
            }),
            vararg: None,
            return_type: None,
            block: Block {
                span: span(),
                stmts: Vec::new(),
                last_stmt: Some(Box::new(LastStatement::Return(Box::new(ReturnStatement {
                    span: span(),
                    exprs: Punctuated::from_item(binop_expr(name_expr("a"), num_expr())),
                })))),
            },
        };
        let block = Block {
            span: span(),
            stmts: vec![Statement::LocalFunction(Box::new(LocalFunction {
                span: span(),
                attributes: Vec::new(),
                is_const: false,
                name: token(TokenKind::Identifier("f".into())),
                body,
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 3);
    }

    #[test]
    fn visit_if_statement() {
        // if x then y = 1 else y = nil end => x, 1, nil = 3 expressions
        let block = Block {
            span: span(),
            stmts: vec![Statement::IfStatement(Box::new(IfStatement {
                span: span(),
                condition: name_expr("x"),
                block: Block {
                    span: span(),
                    stmts: vec![Statement::Assignment(Box::new(Assignment {
                        span: span(),
                        targets: Punctuated::from_item(Var::Name(token(TokenKind::Identifier(
                            "y".into(),
                        )))),
                        values: Punctuated::from_item(num_expr()),
                    }))],
                    last_stmt: None,
                },
                elseif_clauses: Vec::new(),
                else_clause: Some(ElseClause {
                    span: span(),
                    block: Block {
                        span: span(),
                        stmts: vec![Statement::Assignment(Box::new(Assignment {
                            span: span(),
                            targets: Punctuated::from_item(Var::Name(token(
                                TokenKind::Identifier("y".into()),
                            ))),
                            values: Punctuated::from_item(nil_expr()),
                        }))],
                        last_stmt: None,
                    },
                }),
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 3);
    }

    #[test]
    fn visit_numeric_for() {
        // for i = 1, 10, 2 do end => 3 expressions (1, 10, 2)
        let block = Block {
            span: span(),
            stmts: vec![Statement::NumericFor(Box::new(NumericFor {
                span: span(),
                name: token(TokenKind::Identifier("i".into())),
                type_annotation: None,
                start: num_expr(),
                limit: num_expr(),
                step: Some(num_expr()),
                block: empty_block(),
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        assert_eq!(counter.0, 3);
    }

    #[test]
    fn visit_nested_function_call() {
        // f(x) => callee f (1 var expr) + arg x (1 var expr) + whole call (1 call expr) = 3
        let call = FunctionCall {
            span: span(),
            callee: name_expr("f"),
            args: FunctionArgs::Parenthesized {
                span: span(),
                args: Punctuated::from_item(name_expr("x")),
            },
            method: None,
        };
        let block = Block {
            span: span(),
            stmts: vec![Statement::FunctionCall(Box::new(FunctionCallStmt {
                span: span(),
                call,
            }))],
            last_stmt: None,
        };
        let mut counter = ExprCounter(0);
        counter.visit_block(&block);
        // callee "f" + arg "x" = 2 expressions (FunctionCall is a statement here, not an expression)
        assert_eq!(counter.0, 2);
    }
}
