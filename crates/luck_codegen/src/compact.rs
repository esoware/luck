use luck_ast::expr::{
    BinaryOp, Expression, FieldAccess, FunctionArgs, FunctionCall, FunctionDef, IfExpression,
    IndexExpression, InterpolatedString, ParenExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
use luck_ast::shared::{Block, ContainedSpan, Field, FunctionBody, Punctuated};
use luck_ast::stmt::{
    Assignment, AttributedName, CompoundAssignment, DoBlock, FunctionCallStmt, FunctionDecl,
    GenericFor, GlobalDeclaration, GlobalFunction, GlobalStar, GotoStatement, IfStatement,
    LabelStatement, LastStatement, LocalAssignment, LocalFunction, NumericFor, RepeatLoop,
    ReturnStatement, Statement, TypeDeclaration, TypeDeclarationValue, WhileLoop,
};
use luck_ast::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenType, TableType, Type, TypeArgs, TypeField,
    TypePack, TypeofType, UnionType, VariadicType,
};
use luck_token::Span;
use luck_token::token::{Token, TokenKind};

use crate::separator::{self, Separator};

/// Emits an AST as compact Lua code with minimal whitespace.
pub struct CompactPrinter<'src> {
    output: String,
    last_token: Option<TokenKind>,
    source: &'src str,
}

impl<'src> CompactPrinter<'src> {
    pub fn new(source: &'src str) -> Self {
        Self {
            // Capacity hint only: compact output stays at or under source
            // length, and synthetic ASTs (empty source) just start empty.
            output: String::with_capacity(source.len()),
            last_token: None,
            source,
        }
    }

    pub fn output(self) -> String {
        self.output
    }

    fn emit_token(&mut self, token: &Token) {
        let text = token_text(&token.kind, self.source, token.span);
        if text.is_empty() {
            return;
        }
        if let Some(ref prev) = self.last_token
            && separator::needs_separator(prev, &token.kind) == Separator::Space
        {
            self.output.push(' ');
        }
        self.output.push_str(text);
        self.last_token = Some(token.kind.clone());
    }

    pub fn emit_block(&mut self, block: &Block) {
        for (idx, stmt) in block.stmts.iter().enumerate() {
            // Prevent ambiguous statement boundaries: if the previous statement
            // ended with ')' or '}' and this statement starts with '(', the parser
            // would chain them into a single call/index expression.
            if idx > 0
                && luck_ast::query::stmt_starts_with_paren(stmt)
                && !matches!(self.last_token, Some(TokenKind::Semicolon))
            {
                self.output.push(';');
                self.last_token = Some(TokenKind::Semicolon);
            }
            self.emit_statement(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.emit_last_statement(last);
        }
    }

    fn emit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(assign) => self.emit_assignment(assign),
            Statement::FunctionCall(call) => self.emit_function_call_stmt(call),
            Statement::DoBlock(block) => self.emit_do_block(block),
            Statement::WhileLoop(while_loop) => self.emit_while_loop(while_loop),
            Statement::RepeatLoop(repeat_loop) => self.emit_repeat_loop(repeat_loop),
            Statement::IfStatement(if_stmt) => self.emit_if_statement(if_stmt),
            Statement::NumericFor(num_for) => self.emit_numeric_for(num_for),
            Statement::GenericFor(gen_for) => self.emit_generic_for(gen_for),
            Statement::FunctionDecl(func_decl) => self.emit_function_decl(func_decl),
            Statement::LocalFunction(local_fn) => self.emit_local_function(local_fn),
            Statement::LocalAssignment(local_assign) => self.emit_local_assignment(local_assign),
            Statement::EmptyStatement(token) => self.emit_token(token),
            Statement::Goto(goto_stmt) => self.emit_goto(goto_stmt),
            Statement::Label(label) => self.emit_label(label),
            Statement::GlobalDeclaration(global_decl) => self.emit_global_declaration(global_decl),
            Statement::GlobalFunction(global_fn) => self.emit_global_function(global_fn),
            Statement::GlobalStar(global_star) => self.emit_global_star(global_star),
            Statement::Break(token) => self.emit_token(token),
            Statement::CompoundAssignment(compound) => self.emit_compound_assignment(compound),
            Statement::TypeDeclaration(type_decl) => self.emit_type_declaration(type_decl),
            Statement::Error(_) => {}
        }
    }

    fn emit_last_statement(&mut self, stmt: &LastStatement) {
        match stmt {
            LastStatement::Return(ret) => self.emit_return(ret),
            LastStatement::Break(token) | LastStatement::Continue(token) => self.emit_token(token),
            LastStatement::Error(_) => {}
        }
    }

    fn emit_assignment(&mut self, assign: &Assignment) {
        self.emit_punctuated_vars(&assign.targets);
        self.emit_token(&assign.equal);
        self.emit_punctuated_exprs(&assign.values);
    }

    fn emit_function_call_stmt(&mut self, call: &FunctionCallStmt) {
        self.emit_function_call(&call.call);
    }

    fn emit_do_block(&mut self, block: &DoBlock) {
        self.emit_token(&block.do_token);
        self.emit_block(&block.block);
        self.emit_token(&block.end_token);
    }

    fn emit_while_loop(&mut self, while_loop: &WhileLoop) {
        self.emit_token(&while_loop.while_token);
        self.emit_expression(&while_loop.condition);
        self.emit_token(&while_loop.do_token);
        self.emit_block(&while_loop.block);
        self.emit_token(&while_loop.end_token);
    }

    fn emit_repeat_loop(&mut self, repeat_loop: &RepeatLoop) {
        self.emit_token(&repeat_loop.repeat_token);
        self.emit_block(&repeat_loop.block);
        self.emit_token(&repeat_loop.until_token);
        self.emit_expression(&repeat_loop.condition);
    }

    fn emit_if_statement(&mut self, if_stmt: &IfStatement) {
        self.emit_token(&if_stmt.if_token);
        self.emit_expression(&if_stmt.condition);
        self.emit_token(&if_stmt.then_token);
        self.emit_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.emit_token(&clause.elseif_token);
            self.emit_expression(&clause.condition);
            self.emit_token(&clause.then_token);
            self.emit_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.emit_token(&else_clause.else_token);
            self.emit_block(&else_clause.block);
        }
        self.emit_token(&if_stmt.end_token);
    }

    fn emit_numeric_for(&mut self, num_for: &NumericFor) {
        self.emit_token(&num_for.for_token);
        self.emit_token(&num_for.name);
        if let Some((colon, type_value)) = &num_for.type_annotation {
            self.emit_token(colon);
            self.emit_type(type_value);
        }
        self.emit_token(&num_for.equal);
        self.emit_expression(&num_for.start);
        self.emit_token(&num_for.comma1);
        self.emit_expression(&num_for.limit);
        if let Some((comma, step)) = &num_for.comma2_and_step {
            self.emit_token(comma);
            self.emit_expression(step);
        }
        self.emit_token(&num_for.do_token);
        self.emit_block(&num_for.block);
        self.emit_token(&num_for.end_token);
    }

    fn emit_generic_for(&mut self, gen_for: &GenericFor) {
        self.emit_token(&gen_for.for_token);
        for (param, sep) in &gen_for.names.items {
            self.emit_token(&param.name);
            if let Some((colon, type_value)) = &param.type_annotation {
                self.emit_token(colon);
                self.emit_type(type_value);
            }
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
        self.emit_token(&gen_for.in_token);
        self.emit_punctuated_exprs(&gen_for.exprs);
        self.emit_token(&gen_for.do_token);
        self.emit_block(&gen_for.block);
        self.emit_token(&gen_for.end_token);
    }

    fn emit_function_decl(&mut self, func_decl: &FunctionDecl) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        for attr in &func_decl.attributes {
            self.emit_token(&attr.at_token);
            self.emit_token(&attr.name);
        }
        self.emit_token(&func_decl.function_token);
        self.emit_func_name(&func_decl.name);
        self.emit_function_body(&func_decl.body);
    }

    fn emit_func_name(&mut self, name: &luck_ast::stmt::FuncName) {
        for (idx, name_token) in name.names.iter().enumerate() {
            if idx > 0 {
                self.emit_token(&name.dots[idx - 1]);
            }
            self.emit_token(name_token);
        }
        if let Some((colon, method_name)) = &name.method {
            self.emit_token(colon);
            self.emit_token(method_name);
        }
    }

    fn emit_local_function(&mut self, local_fn: &LocalFunction) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        for attr in &local_fn.attributes {
            self.emit_token(&attr.at_token);
            self.emit_token(&attr.name);
        }
        self.emit_token(&local_fn.local_token);
        self.emit_token(&local_fn.function_token);
        self.emit_token(&local_fn.name);
        self.emit_function_body(&local_fn.body);
    }

    fn emit_local_assignment(&mut self, local_assign: &LocalAssignment) {
        self.emit_token(&local_assign.local_token);
        for (attributed, sep) in &local_assign.names.items {
            self.emit_attributed_name(attributed);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
        if let Some((equal, exprs)) = &local_assign.equal_and_exprs {
            self.emit_token(equal);
            self.emit_punctuated_exprs(exprs);
        }
    }

    fn emit_attributed_name(&mut self, attributed: &AttributedName) {
        self.emit_token(&attributed.name);
        if let Some((colon, type_value)) = &attributed.type_annotation {
            self.emit_token(colon);
            self.emit_type(type_value);
        }
        if let Some(attrib) = &attributed.attrib {
            self.emit_token(&attrib.open);
            self.emit_token(&attrib.name);
            self.emit_token(&attrib.close);
        }
    }

    fn emit_goto(&mut self, goto_stmt: &GotoStatement) {
        self.emit_token(&goto_stmt.goto_token);
        self.emit_token(&goto_stmt.name);
    }

    fn emit_label(&mut self, label: &LabelStatement) {
        self.emit_token(&label.colons_open);
        self.emit_token(&label.name);
        self.emit_token(&label.colons_close);
    }

    fn emit_return(&mut self, ret: &ReturnStatement) {
        self.emit_token(&ret.return_token);
        self.emit_punctuated_exprs(&ret.exprs);
    }

    fn emit_compound_assignment(&mut self, compound: &CompoundAssignment) {
        self.emit_var(&compound.var);
        self.emit_token(&compound.op);
        self.emit_expression(&compound.expr);
    }

    fn emit_type_declaration(&mut self, type_decl: &TypeDeclaration) {
        if let Some(export) = &type_decl.export_token {
            self.emit_token(export);
        }
        self.emit_token(&type_decl.type_token);
        // Luau `type function Name funcbody` - no `=`.
        if let Some(function_token) = &type_decl.function_token {
            self.emit_token(function_token);
        }
        self.emit_token(&type_decl.name);
        if let Some(generics) = &type_decl.generics {
            self.emit_generic_type_list(generics);
        }
        if let Some(equal) = &type_decl.equal {
            self.emit_token(equal);
        }
        match &type_decl.type_value {
            TypeDeclarationValue::Alias(type_value) => self.emit_type(type_value),
            // Luau `type function Name funcbody` reuses ordinary function-body emission.
            TypeDeclarationValue::TypeFunction(body) => self.emit_function_body(body),
        }
    }

    fn emit_global_declaration(&mut self, global_decl: &GlobalDeclaration) {
        self.emit_token(&global_decl.global_token);
        for (attributed, sep) in &global_decl.names.items {
            self.emit_attributed_name(attributed);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
    }

    fn emit_global_function(&mut self, global_fn: &GlobalFunction) {
        self.emit_token(&global_fn.global_token);
        self.emit_token(&global_fn.function_token);
        self.emit_token(&global_fn.name);
        self.emit_function_body(&global_fn.body);
    }

    fn emit_global_star(&mut self, global_star: &GlobalStar) {
        self.emit_token(&global_star.global_token);
        if let Some(attrib) = &global_star.attrib {
            self.emit_token(&attrib.open);
            self.emit_token(&attrib.name);
            self.emit_token(&attrib.close);
        }
        self.emit_token(&global_star.star);
    }

    fn emit_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Nil(token)
            | Expression::False(token)
            | Expression::True(token)
            | Expression::Number(token)
            | Expression::StringLiteral(token)
            | Expression::VarArg(token) => self.emit_token(token),
            Expression::FunctionDef(func_def) => self.emit_function_def(func_def),
            Expression::Var(var) => self.emit_var(var),
            Expression::FunctionCall(call) => self.emit_function_call(call),
            Expression::Parenthesized(paren) => self.emit_paren_expression(paren),
            Expression::TableConstructor(table) => self.emit_table_constructor(table),
            Expression::BinaryOp(binop) => self.emit_binary_op(binop),
            Expression::UnaryOp(unop) => self.emit_unary_op(unop),
            Expression::IfExpression(if_expr) => self.emit_if_expression(if_expr),
            Expression::InterpolatedString(interp) => self.emit_interpolated_string(interp),
            Expression::TypeCast(cast) => self.emit_type_cast(cast),
            Expression::Error(_) => {}
        }
    }

    fn emit_var(&mut self, var: &Var) {
        match var {
            Var::Name(token) => self.emit_token(token),
            Var::Index(index) => self.emit_index_expression(index),
            Var::FieldAccess(field) => self.emit_field_access(field),
        }
    }

    fn emit_function_def(&mut self, func_def: &FunctionDef) {
        self.emit_token(&func_def.function_token);
        self.emit_function_body(&func_def.body);
    }

    fn emit_function_call(&mut self, call: &FunctionCall) {
        self.emit_expression(&call.callee);
        if let Some((colon, method_name)) = &call.method {
            self.emit_token(colon);
            self.emit_token(method_name);
        }
        self.emit_function_args(&call.args);
    }

    fn emit_function_args(&mut self, args: &FunctionArgs) {
        match args {
            FunctionArgs::Parenthesized { parens, args } => {
                self.emit_contained_span_open(parens);
                self.emit_punctuated_exprs(args);
                self.emit_contained_span_close(parens);
            }
            FunctionArgs::TableConstructor(table) => self.emit_table_constructor(table),
            FunctionArgs::StringLiteral(token) => self.emit_token(token),
        }
    }

    fn emit_paren_expression(&mut self, paren: &ParenExpression) {
        self.emit_contained_span_open(&paren.parens);
        self.emit_expression(&paren.expr);
        self.emit_contained_span_close(&paren.parens);
    }

    fn emit_table_constructor(&mut self, table: &TableConstructor) {
        self.emit_contained_span_open(&table.braces);
        for (idx, (field, sep)) in table.fields.iter().enumerate() {
            self.emit_field(field);
            let is_last = idx == table.fields.len() - 1;
            if !is_last {
                if let Some(sep_token) = sep {
                    self.emit_token(sep_token);
                } else {
                    // Parser should always produce separators between fields,
                    // but emit a comma as fallback to prevent broken output.
                    self.output.push(',');
                    self.last_token = Some(TokenKind::Comma);
                }
            }
        }
        self.emit_contained_span_close(&table.braces);
    }

    fn emit_field(&mut self, field: &Field) {
        match field {
            Field::Bracketed {
                brackets,
                key,
                equal,
                value,
                ..
            } => {
                self.emit_contained_span_open(brackets);
                self.emit_expression(key);
                self.emit_contained_span_close(brackets);
                self.emit_token(equal);
                self.emit_expression(value);
            }
            Field::Named {
                name, equal, value, ..
            } => {
                self.emit_token(name);
                self.emit_token(equal);
                self.emit_expression(value);
            }
            Field::Positional { value, .. } => {
                self.emit_expression(value);
            }
        }
    }

    fn emit_index_expression(&mut self, index: &IndexExpression) {
        self.emit_expression(&index.prefix);
        self.emit_contained_span_open(&index.brackets);
        self.emit_expression(&index.index);
        self.emit_contained_span_close(&index.brackets);
    }

    fn emit_field_access(&mut self, field: &FieldAccess) {
        self.emit_expression(&field.prefix);
        self.emit_token(&field.dot);
        self.emit_token(&field.name);
    }

    fn emit_binary_op(&mut self, binop: &BinaryOp) {
        self.emit_expression(&binop.left);
        self.emit_token(&binop.op);
        self.emit_expression(&binop.right);
    }

    fn emit_unary_op(&mut self, unop: &UnaryOp) {
        self.emit_token(&unop.op);
        self.emit_expression(&unop.operand);
    }

    fn emit_if_expression(&mut self, if_expr: &IfExpression) {
        self.emit_token(&if_expr.if_token);
        self.emit_expression(&if_expr.condition);
        self.emit_token(&if_expr.then_token);
        self.emit_expression(&if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            self.emit_token(&clause.elseif_token);
            self.emit_expression(&clause.condition);
            self.emit_token(&clause.then_token);
            self.emit_expression(&clause.expr);
        }
        self.emit_token(&if_expr.else_token);
        self.emit_expression(&if_expr.else_expr);
    }

    fn emit_interpolated_string(&mut self, interp: &InterpolatedString) {
        for segment in &interp.segments {
            self.emit_interp_token(&segment.literal);
            if let Some(expr) = &segment.expr {
                self.emit_expression(expr);
            }
        }
    }

    /// Emit an interpolated string token with proper backtick/brace delimiters.
    /// InterpBegin("text") -> `` `text{ ``
    /// InterpMid("text") -> `}text{`
    /// InterpEnd("text") -> `` }text` ``
    fn emit_interp_token(&mut self, token: &Token) {
        let text = match &token.kind {
            TokenKind::InterpBegin(s) => format!("`{s}{{"),
            TokenKind::InterpMid(s) => format!("}}{s}{{"),
            TokenKind::InterpEnd(s) => format!("}}{s}`"),
            _ => {
                self.emit_token(token);
                return;
            }
        };
        self.output.push_str(&text);
        self.last_token = Some(token.kind.clone());
    }

    fn emit_type_cast(&mut self, cast: &TypeCast) {
        self.emit_expression(&cast.expr);
        self.emit_token(&cast.double_colon);
        self.emit_type(&cast.type_annotation);
    }

    fn emit_function_body(&mut self, body: &FunctionBody) {
        // Luau: `<T, U...>` generics sit between the function name and `(`.
        if let Some(generics) = &body.generics {
            self.emit_generic_type_list(generics);
        }
        self.emit_contained_span_open(&body.params_parens);
        for (param, sep) in &body.params.items {
            self.emit_token(&param.name);
            if let Some((colon, type_value)) = &param.type_annotation {
                self.emit_token(colon);
                self.emit_type(type_value);
            }
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
        if let Some(vararg) = &body.vararg {
            self.emit_token(&vararg.dots);
            if let Some(name) = &vararg.name {
                self.emit_token(name);
            }
            if let Some((colon, type_value)) = &vararg.type_annotation {
                self.emit_token(colon);
                self.emit_type(type_value);
            }
        }
        self.emit_contained_span_close(&body.params_parens);
        if let Some((colon, return_type)) = &body.return_type {
            self.emit_token(colon);
            self.emit_type(return_type);
        }
        self.emit_block(&body.block);
        self.emit_token(&body.end_token);
    }

    fn emit_contained_span_open(&mut self, span: &ContainedSpan) {
        self.emit_token(&span.open);
    }

    fn emit_contained_span_close(&mut self, span: &ContainedSpan) {
        self.emit_token(&span.close);
    }

    fn emit_punctuated_exprs(&mut self, punct: &Punctuated<Expression>) {
        for (expr, sep) in &punct.items {
            self.emit_expression(expr);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
    }

    fn emit_punctuated_vars(&mut self, punct: &Punctuated<Var>) {
        for (var, sep) in &punct.items {
            self.emit_var(var);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
    }

    fn emit_type(&mut self, ty: &Type) {
        match ty {
            Type::Named(named) => self.emit_named_type(named),
            Type::Typeof(typeof_type) => self.emit_typeof_type(typeof_type),
            Type::Table(table) => self.emit_table_type(table),
            Type::Function(function) => self.emit_function_type(function),
            Type::Optional(optional) => self.emit_optional_type(optional),
            Type::Union(union) => self.emit_union_type(union),
            Type::Intersection(intersection) => self.emit_intersection_type(intersection),
            Type::Parenthesized(paren) => self.emit_paren_type(paren),
            Type::Pack(pack) => self.emit_type_pack(pack),
            Type::Singleton(token) => self.emit_token(token),
            Type::Variadic(variadic) => self.emit_variadic_type(variadic),
            Type::GenericPack(generic_pack) => self.emit_generic_pack_type(generic_pack),
            // Mirrors `Statement::Error` / `Expression::Error`: emit nothing.
            Type::Error(_) => {}
        }
    }

    fn emit_named_type(&mut self, named: &NamedType) {
        if let Some((module, dot)) = &named.prefix {
            self.emit_token(module);
            self.emit_token(dot);
        }
        self.emit_token(&named.name);
        if let Some(generics) = &named.generics {
            self.emit_type_args(generics);
        }
    }

    fn emit_type_args(&mut self, args: &TypeArgs) {
        self.emit_contained_span_open(&args.angles);
        self.emit_punctuated_types(&args.args);
        self.emit_contained_span_close(&args.angles);
    }

    fn emit_typeof_type(&mut self, typeof_type: &TypeofType) {
        self.emit_token(&typeof_type.typeof_token);
        self.emit_contained_span_open(&typeof_type.parens);
        self.emit_expression(&typeof_type.expr);
        self.emit_contained_span_close(&typeof_type.parens);
    }

    fn emit_table_type(&mut self, table: &TableType) {
        self.emit_contained_span_open(&table.braces);
        for (idx, (field, sep)) in table.fields.iter().enumerate() {
            self.emit_type_field(field);
            let is_last = idx == table.fields.len() - 1;
            if !is_last {
                if let Some(sep_token) = sep {
                    self.emit_token(sep_token);
                } else {
                    // Parser should always produce separators between fields,
                    // but emit a comma as fallback to prevent broken output.
                    self.output.push(',');
                    self.last_token = Some(TokenKind::Comma);
                }
            }
        }
        self.emit_contained_span_close(&table.braces);
    }

    fn emit_type_field(&mut self, field: &TypeField) {
        match field {
            TypeField::Named {
                access,
                name,
                colon,
                value,
                ..
            } => {
                // Luau `read`/`write` - a word, so the separator spaces it from the name.
                if let Some(access) = access {
                    self.emit_token(access);
                }
                self.emit_token(name);
                self.emit_token(colon);
                self.emit_type(value);
            }
            TypeField::Indexer {
                access,
                brackets,
                key,
                colon,
                value,
                ..
            } => {
                if let Some(access) = access {
                    self.emit_token(access);
                }
                self.emit_contained_span_open(brackets);
                self.emit_type(key);
                self.emit_contained_span_close(brackets);
                self.emit_token(colon);
                self.emit_type(value);
            }
            TypeField::Array { value, .. } => {
                self.emit_type(value);
            }
        }
    }

    fn emit_function_type(&mut self, function: &FunctionType) {
        if let Some(generics) = &function.generics {
            self.emit_generic_type_list(generics);
        }
        self.emit_contained_span_open(&function.parens);
        for (param, sep) in &function.params.items {
            self.emit_function_type_param(param);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
        self.emit_contained_span_close(&function.parens);
        self.emit_token(&function.arrow);
        self.emit_type(&function.return_type);
    }

    fn emit_function_type_param(&mut self, param: &FunctionTypeParam) {
        if let Some((name, colon)) = &param.name {
            self.emit_token(name);
            self.emit_token(colon);
        }
        self.emit_type(&param.type_value);
    }

    fn emit_optional_type(&mut self, optional: &OptionalType) {
        self.emit_type(&optional.type_value);
        self.emit_token(&optional.question);
    }

    fn emit_union_type(&mut self, union: &UnionType) {
        if let Some(pipe) = &union.leading_pipe {
            self.emit_token(pipe);
        }
        self.emit_punctuated_types(&union.types);
    }

    fn emit_intersection_type(&mut self, intersection: &IntersectionType) {
        if let Some(ampersand) = &intersection.leading_ampersand {
            self.emit_token(ampersand);
        }
        self.emit_punctuated_types(&intersection.types);
    }

    fn emit_paren_type(&mut self, paren: &ParenType) {
        self.emit_contained_span_open(&paren.parens);
        self.emit_type(&paren.type_value);
        self.emit_contained_span_close(&paren.parens);
    }

    fn emit_type_pack(&mut self, pack: &TypePack) {
        self.emit_contained_span_open(&pack.parens);
        self.emit_punctuated_types(&pack.types);
        self.emit_contained_span_close(&pack.parens);
    }

    fn emit_variadic_type(&mut self, variadic: &VariadicType) {
        self.emit_token(&variadic.dots);
        self.emit_type(&variadic.type_value);
    }

    fn emit_generic_pack_type(&mut self, generic_pack: &GenericPackType) {
        self.emit_token(&generic_pack.name);
        self.emit_token(&generic_pack.dots);
    }

    fn emit_generic_type_list(&mut self, generics: &GenericTypeList) {
        self.emit_contained_span_open(&generics.angles);
        for (param, sep) in &generics.params.items {
            self.emit_generic_type_param(param);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
        self.emit_contained_span_close(&generics.angles);
    }

    fn emit_generic_type_param(&mut self, param: &GenericTypeParam) {
        self.emit_token(&param.name);
        if let Some(dots) = &param.dots {
            self.emit_token(dots);
        }
        if let Some((equal, default)) = &param.default {
            self.emit_token(equal);
            self.emit_type(default);
        }
    }

    fn emit_punctuated_types(&mut self, punct: &Punctuated<Type>) {
        for (ty, sep) in &punct.items {
            self.emit_type(ty);
            if let Some(sep) = sep {
                self.emit_token(sep);
            }
        }
    }
}

fn token_text<'a>(kind: &'a TokenKind, _source: &'a str, _span: Span) -> &'a str {
    match kind {
        TokenKind::Identifier(s) | TokenKind::Number(s) | TokenKind::StringLiteral(s) => s,
        TokenKind::InterpBegin(s) | TokenKind::InterpMid(s) | TokenKind::InterpEnd(s) => s,
        TokenKind::And => "and",
        TokenKind::Break => "break",
        TokenKind::Do => "do",
        TokenKind::Else => "else",
        TokenKind::ElseIf => "elseif",
        TokenKind::End => "end",
        TokenKind::False => "false",
        TokenKind::For => "for",
        TokenKind::Function => "function",
        TokenKind::Goto => "goto",
        TokenKind::Global => "global",
        TokenKind::If => "if",
        TokenKind::In => "in",
        TokenKind::Local => "local",
        TokenKind::Nil => "nil",
        TokenKind::Not => "not",
        TokenKind::Or => "or",
        TokenKind::Repeat => "repeat",
        TokenKind::Return => "return",
        TokenKind::Then => "then",
        TokenKind::True => "true",
        TokenKind::Until => "until",
        TokenKind::While => "while",
        TokenKind::Plus => "+",
        TokenKind::Minus => "-",
        TokenKind::Star => "*",
        TokenKind::Slash => "/",
        TokenKind::FloorDiv => "//",
        TokenKind::Percent => "%",
        TokenKind::Caret => "^",
        TokenKind::Hash => "#",
        TokenKind::Ampersand => "&",
        TokenKind::Tilde => "~",
        TokenKind::Pipe => "|",
        TokenKind::ShiftLeft => "<<",
        TokenKind::ShiftRight => ">>",
        TokenKind::Dot => ".",
        TokenKind::DotDot => "..",
        TokenKind::DotDotDot => "...",
        TokenKind::Semicolon => ";",
        TokenKind::Colon => ":",
        TokenKind::DoubleColon => "::",
        TokenKind::Comma => ",",
        TokenKind::Equal => "=",
        TokenKind::EqualEqual => "==",
        TokenKind::TildeEqual => "~=",
        TokenKind::Less => "<",
        TokenKind::LessEqual => "<=",
        TokenKind::Greater => ">",
        TokenKind::GreaterEqual => ">=",
        TokenKind::LeftParen => "(",
        TokenKind::RightParen => ")",
        TokenKind::LeftBrace => "{",
        TokenKind::RightBrace => "}",
        TokenKind::LeftBracket => "[",
        TokenKind::RightBracket => "]",
        TokenKind::PlusEqual => "+=",
        TokenKind::MinusEqual => "-=",
        TokenKind::StarEqual => "*=",
        TokenKind::SlashEqual => "/=",
        TokenKind::FloorDivEqual => "//=",
        TokenKind::PercentEqual => "%=",
        TokenKind::CaretEqual => "^=",
        TokenKind::DotDotEqual => "..=",
        // Luau symbols
        TokenKind::At => "@",
        TokenKind::Arrow => "->",
        TokenKind::Question => "?",
        TokenKind::Eof => "",
    }
}
