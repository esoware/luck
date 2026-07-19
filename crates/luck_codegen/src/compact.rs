use luck_ast::expr::{
    BinaryOp, Expression, FieldAccess, FunctionArgs, FunctionCall, FunctionDef, IfExpression,
    IndexExpression, InterpolatedString, ParenExpression, TableConstructor, TypeCast, UnaryOp, Var,
};
use luck_ast::shared::{Block, Field, FunctionBody, Punctuated};
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
use luck_token::code_buffer::CodeBuffer;
use luck_token::token::{Token, TokenKind};

use crate::separator::{PrevClass, classify_str, is_word_text, needs_space};

/// Emits an AST as compact Lua code with minimal whitespace.
pub struct CompactPrinter {
    output: CodeBuffer,
    prev: PrevClass,
}

impl CompactPrinter {
    pub fn new(source: &str) -> Self {
        Self {
            // Capacity hint only: compact output stays at or under source
            // length, and synthetic ASTs (empty source) just start empty.
            output: CodeBuffer::with_capacity(source.len()),
            prev: PrevClass::None,
        }
    }

    pub fn output(self) -> String {
        self.output.into_string()
    }

    /// Print one piece with its separator decision. Every emit path funnels
    /// through here so `prev` stays a single byte of state.
    fn emit_piece(&mut self, text: &str, is_wordlike: bool, class: PrevClass) {
        let Some(&first) = text.as_bytes().first() else {
            return;
        };
        if needs_space(self.prev, first, is_wordlike) {
            self.output.print_ascii_byte(b' ');
        }
        self.output.print_str(text);
        self.prev = class;
    }

    /// Emit a fixed-spelling piece (keyword, operator, punctuation).
    fn emit_str(&mut self, text: &'static str) {
        self.emit_piece(text, is_word_text(text), classify_str(text));
    }

    /// Emit a payload-carrying token (identifier, number, string literal).
    /// Fixed-spelling kinds (`Type::Singleton` carries `nil`/`true`/`false`)
    /// fall through to their static text.
    fn emit_token(&mut self, token: &Token) {
        match &token.kind {
            TokenKind::Identifier(text) => self.emit_piece(text, true, PrevClass::Word),
            TokenKind::Number(text) => self.emit_piece(text, true, PrevClass::Number),
            TokenKind::StringLiteral(text) => self.emit_piece(text, false, PrevClass::Other),
            TokenKind::Nil => self.emit_str("nil"),
            TokenKind::True => self.emit_str("true"),
            TokenKind::False => self.emit_str("false"),
            other => {
                debug_assert!(false, "unexpected token kind in AST: {other:?}");
            }
        }
    }

    pub fn emit_block(&mut self, block: &Block) {
        for (idx, stmt) in block.stmts.iter().enumerate() {
            // Prevent ambiguous statement boundaries: if the previous statement
            // ended with ')' or '}' and this statement starts with '(', the parser
            // would chain them into a single call/index expression.
            if idx > 0
                && luck_ast::query::stmt_starts_with_paren(stmt)
                && self.prev != PrevClass::Semicolon
            {
                self.output.print_ascii_byte(b';');
                self.prev = PrevClass::Semicolon;
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
            Statement::EmptyStatement(_) => self.emit_str(";"),
            Statement::Goto(goto_stmt) => self.emit_goto(goto_stmt),
            Statement::Label(label) => self.emit_label(label),
            Statement::GlobalDeclaration(global_decl) => self.emit_global_declaration(global_decl),
            Statement::GlobalFunction(global_fn) => self.emit_global_function(global_fn),
            Statement::GlobalStar(global_star) => self.emit_global_star(global_star),
            Statement::Break(_) => self.emit_str("break"),
            Statement::CompoundAssignment(compound) => self.emit_compound_assignment(compound),
            Statement::TypeDeclaration(type_decl) => self.emit_type_declaration(type_decl),
            Statement::Error(_) => {}
        }
    }

    fn emit_last_statement(&mut self, stmt: &LastStatement) {
        match stmt {
            LastStatement::Return(ret) => self.emit_return(ret),
            LastStatement::Break(_) => self.emit_str("break"),
            LastStatement::Continue(_) => self.emit_str("continue"),
            LastStatement::Error(_) => {}
        }
    }

    fn emit_assignment(&mut self, assign: &Assignment) {
        self.emit_punctuated_vars(&assign.targets);
        self.emit_str("=");
        self.emit_punctuated_exprs(&assign.values);
    }

    fn emit_function_call_stmt(&mut self, call: &FunctionCallStmt) {
        self.emit_function_call(&call.call);
    }

    fn emit_do_block(&mut self, block: &DoBlock) {
        self.emit_str("do");
        self.emit_block(&block.block);
        self.emit_str("end");
    }

    fn emit_while_loop(&mut self, while_loop: &WhileLoop) {
        self.emit_str("while");
        self.emit_expression(&while_loop.condition);
        self.emit_str("do");
        self.emit_block(&while_loop.block);
        self.emit_str("end");
    }

    fn emit_repeat_loop(&mut self, repeat_loop: &RepeatLoop) {
        self.emit_str("repeat");
        self.emit_block(&repeat_loop.block);
        self.emit_str("until");
        self.emit_expression(&repeat_loop.condition);
    }

    fn emit_if_statement(&mut self, if_stmt: &IfStatement) {
        self.emit_str("if");
        self.emit_expression(&if_stmt.condition);
        self.emit_str("then");
        self.emit_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.emit_str("elseif");
            self.emit_expression(&clause.condition);
            self.emit_str("then");
            self.emit_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.emit_str("else");
            self.emit_block(&else_clause.block);
        }
        self.emit_str("end");
    }

    fn emit_numeric_for(&mut self, num_for: &NumericFor) {
        self.emit_str("for");
        self.emit_token(&num_for.name);
        if let Some((_, type_value)) = &num_for.type_annotation {
            self.emit_str(":");
            self.emit_type(type_value);
        }
        self.emit_str("=");
        self.emit_expression(&num_for.start);
        self.emit_str(",");
        self.emit_expression(&num_for.limit);
        if let Some((_, step)) = &num_for.comma2_and_step {
            self.emit_str(",");
            self.emit_expression(step);
        }
        self.emit_str("do");
        self.emit_block(&num_for.block);
        self.emit_str("end");
    }

    fn emit_generic_for(&mut self, gen_for: &GenericFor) {
        self.emit_str("for");
        for (param, sep) in &gen_for.names.items {
            self.emit_token(&param.name);
            if let Some((_, type_value)) = &param.type_annotation {
                self.emit_str(":");
                self.emit_type(type_value);
            }
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        self.emit_str("in");
        self.emit_punctuated_exprs(&gen_for.exprs);
        self.emit_str("do");
        self.emit_block(&gen_for.block);
        self.emit_str("end");
    }

    fn emit_function_decl(&mut self, func_decl: &FunctionDecl) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        self.emit_function_attributes(&func_decl.attributes);
        self.emit_str("function");
        self.emit_func_name(&func_decl.name);
        self.emit_function_body(&func_decl.body);
    }

    fn emit_func_name(&mut self, name: &luck_ast::stmt::FuncName) {
        for (idx, name_token) in name.names.iter().enumerate() {
            if idx > 0 {
                self.emit_str(".");
            }
            self.emit_token(name_token);
        }
        if let Some((_, method_name)) = &name.method {
            self.emit_str(":");
            self.emit_token(method_name);
        }
    }

    fn emit_local_function(&mut self, local_fn: &LocalFunction) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        self.emit_function_attributes(&local_fn.attributes);
        self.emit_str(if local_fn.is_const { "const" } else { "local" });
        self.emit_str("function");
        self.emit_token(&local_fn.name);
        self.emit_function_body(&local_fn.body);
    }

    /// Emit the Luau attribute list. Arguments only exist on the
    /// bracketed form, so attributes with args re-emit as `@[name(...)]`.
    fn emit_function_attributes(&mut self, attributes: &[luck_ast::stmt::FunctionAttribute]) {
        for attr in attributes {
            self.emit_str("@");
            if let Some(args) = &attr.args {
                self.emit_str("[");
                self.emit_token(&attr.name);
                self.emit_str("(");
                self.emit_punctuated_exprs(args);
                self.emit_str(")");
                self.emit_str("]");
            } else {
                self.emit_token(&attr.name);
            }
        }
    }

    fn emit_local_assignment(&mut self, local_assign: &LocalAssignment) {
        self.emit_str(if local_assign.is_const {
            "const"
        } else {
            "local"
        });
        for (attributed, sep) in &local_assign.names.items {
            self.emit_attributed_name(attributed);
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        if let Some((_, exprs)) = &local_assign.equal_and_exprs {
            self.emit_str("=");
            self.emit_punctuated_exprs(exprs);
        }
    }

    fn emit_attributed_name(&mut self, attributed: &AttributedName) {
        self.emit_token(&attributed.name);
        if let Some((_, type_value)) = &attributed.type_annotation {
            self.emit_str(":");
            self.emit_type(type_value);
        }
        if let Some(attrib) = &attributed.attrib {
            self.emit_str("<");
            self.emit_token(&attrib.name);
            self.emit_str(">");
        }
    }

    fn emit_goto(&mut self, goto_stmt: &GotoStatement) {
        self.emit_str("goto");
        self.emit_token(&goto_stmt.name);
    }

    fn emit_label(&mut self, label: &LabelStatement) {
        self.emit_str("::");
        self.emit_token(&label.name);
        self.emit_str("::");
    }

    fn emit_return(&mut self, ret: &ReturnStatement) {
        self.emit_str("return");
        self.emit_punctuated_exprs(&ret.exprs);
    }

    fn emit_compound_assignment(&mut self, compound: &CompoundAssignment) {
        self.emit_var(&compound.var);
        self.emit_str(compound.op.static_text());
        self.emit_expression(&compound.expr);
    }

    fn emit_type_declaration(&mut self, type_decl: &TypeDeclaration) {
        if type_decl.export_token.is_some() {
            self.emit_str("export");
        }
        self.emit_str("type");
        // Luau `type function Name funcbody` - no `=`.
        if type_decl.function_token.is_some() {
            self.emit_str("function");
        }
        self.emit_token(&type_decl.name);
        if let Some(generics) = &type_decl.generics {
            self.emit_generic_type_list(generics);
        }
        if type_decl.equal.is_some() {
            self.emit_str("=");
        }
        match &type_decl.type_value {
            TypeDeclarationValue::Alias(type_value) => self.emit_type(type_value),
            // Luau `type function Name funcbody` reuses ordinary function-body emission.
            TypeDeclarationValue::TypeFunction(body) => self.emit_function_body(body),
        }
    }

    fn emit_global_declaration(&mut self, global_decl: &GlobalDeclaration) {
        self.emit_str("global");
        for (attributed, sep) in &global_decl.names.items {
            self.emit_attributed_name(attributed);
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        if let Some((_, exprs)) = &global_decl.equal_and_exprs {
            self.emit_str("=");
            self.emit_punctuated_exprs(exprs);
        }
    }

    fn emit_global_function(&mut self, global_fn: &GlobalFunction) {
        self.emit_str("global");
        self.emit_str("function");
        self.emit_token(&global_fn.name);
        self.emit_function_body(&global_fn.body);
    }

    fn emit_global_star(&mut self, global_star: &GlobalStar) {
        self.emit_str("global");
        if let Some(attrib) = &global_star.attrib {
            self.emit_str("<");
            self.emit_token(&attrib.name);
            self.emit_str(">");
        }
        self.emit_str("*");
    }

    fn emit_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Nil(_) => self.emit_str("nil"),
            Expression::False(_) => self.emit_str("false"),
            Expression::True(_) => self.emit_str("true"),
            Expression::VarArg(_) => self.emit_str("..."),
            Expression::Number(token) | Expression::StringLiteral(token) => self.emit_token(token),
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
        self.emit_function_attributes(&func_def.attributes);
        self.emit_str("function");
        self.emit_function_body(&func_def.body);
    }

    fn emit_function_call(&mut self, call: &FunctionCall) {
        self.emit_expression(&call.callee);
        if let Some((_, method_name)) = &call.method {
            self.emit_str(":");
            self.emit_token(method_name);
        }
        self.emit_function_args(&call.args);
    }

    fn emit_function_args(&mut self, args: &FunctionArgs) {
        match args {
            FunctionArgs::Parenthesized { args, .. } => {
                self.emit_str("(");
                self.emit_punctuated_exprs(args);
                self.emit_str(")");
            }
            FunctionArgs::TableConstructor(table) => self.emit_table_constructor(table),
            FunctionArgs::StringLiteral(token) => self.emit_token(token),
        }
    }

    fn emit_paren_expression(&mut self, paren: &ParenExpression) {
        self.emit_str("(");
        self.emit_expression(&paren.expr);
        self.emit_str(")");
    }

    fn emit_table_constructor(&mut self, table: &TableConstructor) {
        self.emit_str("{");
        for (idx, (field, _)) in table.fields.iter().enumerate() {
            self.emit_field(field);
            if idx + 1 < table.fields.len() {
                self.emit_str(",");
            }
        }
        self.emit_str("}");
    }

    fn emit_field(&mut self, field: &Field) {
        match field {
            Field::Bracketed { key, value, .. } => {
                self.emit_str("[");
                self.emit_expression(key);
                self.emit_str("]");
                self.emit_str("=");
                self.emit_expression(value);
            }
            Field::Named { name, value, .. } => {
                self.emit_token(name);
                self.emit_str("=");
                self.emit_expression(value);
            }
            Field::Positional { value, .. } => {
                self.emit_expression(value);
            }
        }
    }

    fn emit_index_expression(&mut self, index: &IndexExpression) {
        self.emit_expression(&index.prefix);
        self.emit_str("[");
        self.emit_expression(&index.index);
        self.emit_str("]");
    }

    fn emit_field_access(&mut self, field: &FieldAccess) {
        self.emit_expression(&field.prefix);
        self.emit_str(".");
        self.emit_token(&field.name);
    }

    fn emit_binary_op(&mut self, binop: &BinaryOp) {
        self.emit_expression(&binop.left);
        self.emit_str(binop.op.static_text());
        self.emit_expression(&binop.right);
    }

    fn emit_unary_op(&mut self, unop: &UnaryOp) {
        self.emit_str(unop.op.static_text());
        self.emit_expression(&unop.operand);
    }

    fn emit_if_expression(&mut self, if_expr: &IfExpression) {
        self.emit_str("if");
        self.emit_expression(&if_expr.condition);
        self.emit_str("then");
        self.emit_expression(&if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            self.emit_str("elseif");
            self.emit_expression(&clause.condition);
            self.emit_str("then");
            self.emit_expression(&clause.expr);
        }
        self.emit_str("else");
        self.emit_expression(&if_expr.else_expr);
    }

    fn emit_interpolated_string(&mut self, interp: &InterpolatedString) {
        for segment in &interp.segments {
            self.emit_interp_token(&segment.literal);
            if let Some(expr) = &segment.expr {
                // `{{` is a parse error in Luau: an expression starting
                // with a table constructor needs a space after the
                // interpolation opener.
                if luck_ast::query::expr_starts_with_brace(expr) {
                    self.output.print_ascii_byte(b' ');
                }
                self.emit_expression(expr);
            }
        }
    }

    /// Emit an interpolated string token with proper backtick/brace delimiters.
    /// InterpBegin("text") -> `` `text{ ``
    /// InterpMid("text") -> `}text{`
    /// InterpEnd("text") -> `` }text` ``
    fn emit_interp_token(&mut self, token: &Token) {
        let (open, text, close) = match &token.kind {
            TokenKind::InterpBegin(s) => (b'`', s, b'{'),
            TokenKind::InterpMid(s) => (b'}', s, b'{'),
            TokenKind::InterpEnd(s) => (b'}', s, b'`'),
            _ => {
                self.emit_token(token);
                return;
            }
        };
        self.output.print_ascii_byte(open);
        self.output.print_str(text);
        self.output.print_ascii_byte(close);
        self.prev = PrevClass::Other;
    }

    fn emit_type_cast(&mut self, cast: &TypeCast) {
        self.emit_expression(&cast.expr);
        self.emit_str("::");
        self.emit_type(&cast.type_annotation);
    }

    fn emit_function_body(&mut self, body: &FunctionBody) {
        // Luau: `<T, U...>` generics sit between the function name and `(`.
        if let Some(generics) = &body.generics {
            self.emit_generic_type_list(generics);
        }
        self.emit_str("(");
        for (param, sep) in &body.params.items {
            self.emit_token(&param.name);
            if let Some((_, type_value)) = &param.type_annotation {
                self.emit_str(":");
                self.emit_type(type_value);
            }
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        if let Some(vararg) = &body.vararg {
            self.emit_str("...");
            if let Some(name) = &vararg.name {
                self.emit_token(name);
            }
            if let Some((_, type_value)) = &vararg.type_annotation {
                self.emit_str(":");
                self.emit_type(type_value);
            }
        }
        self.emit_str(")");
        if let Some((_, return_type)) = &body.return_type {
            self.emit_str(":");
            self.emit_type(return_type);
        }
        self.emit_block(&body.block);
        self.emit_str("end");
    }

    fn emit_punctuated_exprs(&mut self, punct: &Punctuated<Expression>) {
        for (expr, sep) in &punct.items {
            self.emit_expression(expr);
            if sep.is_some() {
                self.emit_str(",");
            }
        }
    }

    fn emit_punctuated_vars(&mut self, punct: &Punctuated<Var>) {
        for (var, sep) in &punct.items {
            self.emit_var(var);
            if sep.is_some() {
                self.emit_str(",");
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
        if let Some((module, _)) = &named.prefix {
            self.emit_token(module);
            self.emit_str(".");
        }
        self.emit_token(&named.name);
        if let Some(generics) = &named.generics {
            self.emit_type_args(generics);
        }
    }

    fn emit_type_args(&mut self, args: &TypeArgs) {
        self.emit_str("<");
        self.emit_punctuated_types(&args.args);
        self.emit_str(">");
    }

    fn emit_typeof_type(&mut self, typeof_type: &TypeofType) {
        self.emit_str("typeof");
        self.emit_str("(");
        self.emit_expression(&typeof_type.expr);
        self.emit_str(")");
    }

    fn emit_table_type(&mut self, table: &TableType) {
        self.emit_str("{");
        for (idx, (field, _)) in table.fields.iter().enumerate() {
            self.emit_type_field(field);
            if idx + 1 < table.fields.len() {
                self.emit_str(",");
            }
        }
        self.emit_str("}");
    }

    fn emit_type_field(&mut self, field: &TypeField) {
        match field {
            TypeField::Named {
                access,
                name,
                value,
                ..
            } => {
                // Luau `read`/`write` - a word, so the separator spaces it from the name.
                if let Some(access) = access {
                    self.emit_token(access);
                }
                self.emit_token(name);
                self.emit_str(":");
                self.emit_type(value);
            }
            TypeField::Indexer {
                access, key, value, ..
            } => {
                if let Some(access) = access {
                    self.emit_token(access);
                }
                self.emit_str("[");
                self.emit_type(key);
                self.emit_str("]");
                self.emit_str(":");
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
        self.emit_str("(");
        for (param, sep) in &function.params.items {
            self.emit_function_type_param(param);
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        self.emit_str(")");
        self.emit_str("->");
        self.emit_type(&function.return_type);
    }

    fn emit_function_type_param(&mut self, param: &FunctionTypeParam) {
        if let Some((name, _)) = &param.name {
            self.emit_token(name);
            self.emit_str(":");
        }
        self.emit_type(&param.type_value);
    }

    fn emit_optional_type(&mut self, optional: &OptionalType) {
        self.emit_type(&optional.type_value);
        self.emit_str("?");
    }

    fn emit_union_type(&mut self, union: &UnionType) {
        if union.leading_pipe.is_some() {
            self.emit_str("|");
        }
        self.emit_punctuated_types_with(&union.types, "|");
    }

    fn emit_intersection_type(&mut self, intersection: &IntersectionType) {
        if intersection.leading_ampersand.is_some() {
            self.emit_str("&");
        }
        self.emit_punctuated_types_with(&intersection.types, "&");
    }

    fn emit_paren_type(&mut self, paren: &ParenType) {
        self.emit_str("(");
        self.emit_type(&paren.type_value);
        self.emit_str(")");
    }

    fn emit_type_pack(&mut self, pack: &TypePack) {
        self.emit_str("(");
        self.emit_punctuated_types(&pack.types);
        self.emit_str(")");
    }

    fn emit_variadic_type(&mut self, variadic: &VariadicType) {
        self.emit_str("...");
        self.emit_type(&variadic.type_value);
    }

    fn emit_generic_pack_type(&mut self, generic_pack: &GenericPackType) {
        self.emit_token(&generic_pack.name);
        self.emit_str("...");
    }

    fn emit_generic_type_list(&mut self, generics: &GenericTypeList) {
        self.emit_str("<");
        for (param, sep) in &generics.params.items {
            self.emit_generic_type_param(param);
            if sep.is_some() {
                self.emit_str(",");
            }
        }
        self.emit_str(">");
    }

    fn emit_generic_type_param(&mut self, param: &GenericTypeParam) {
        self.emit_token(&param.name);
        if param.dots.is_some() {
            self.emit_str("...");
        }
        if let Some((_, default)) = &param.default {
            self.emit_str("=");
            self.emit_type(default);
        }
    }

    fn emit_punctuated_types(&mut self, punct: &Punctuated<Type>) {
        self.emit_punctuated_types_with(punct, ",");
    }

    /// Type lists reuse `Punctuated`, but the separator spelling depends on
    /// the owning node: `,` for packs and generic args, `|`/`&` for
    /// unions/intersections.
    fn emit_punctuated_types_with(&mut self, punct: &Punctuated<Type>, separator: &'static str) {
        for (ty, sep) in &punct.items {
            self.emit_type(ty);
            if sep.is_some() {
                self.emit_str(separator);
            }
        }
    }
}
