use luck_ast::{
    Assignment, AttributedName, BinaryOp, Block, CompoundAssignment, DoBlock, Expression, Field,
    FieldAccess, FunctionArgs, FunctionAttribute, FunctionBody, FunctionCall, FunctionDecl,
    FunctionDef, FunctionName, FunctionType, FunctionTypeParam, GenericFor, GenericPackType,
    GenericTypeList, GenericTypeParam, GlobalDeclaration, GlobalFunction, GlobalStar,
    GotoStatement, IfExpression, IfStatement, IndexExpression, InterpolatedString,
    IntersectionType, LabelStatement, LastStatement, LocalAssignment, LocalFunction, NamedType,
    NumericFor, OptionalType, Parameter, ParenthesizedExpression, ParenthesizedType, Punctuated,
    RepeatLoop, ReturnStatement, Statement, TableConstructor, TableType, Type, TypeArgs, TypeCast,
    TypeDeclaration, TypeDeclarationValue, TypeField, TypeInstantiation, TypePack, TypeofType,
    UnaryOp, UnionType, Var, VariadicType, WhileLoop, expr_starts_with_brace,
    stmt_starts_with_paren,
};
use luck_token::{CodeBuffer, Token, TokenKind};

use crate::separator::{PieceClass, needs_space};

/// Prints `block` as compact Lua, with no comments and minimal whitespace.
///
/// The output starts with no reserved capacity. Use
/// [`compact_with_capacity`] when there's a useful size estimate.
#[must_use]
pub fn compact(block: &Block) -> String {
    compact_with_capacity(block, 0)
}

/// Like [`compact()`], but with an initial output capacity in bytes.
///
/// The hint only affects allocation, never the output. A previous output's
/// length makes a good one. Source length over-reserves for comment-heavy
/// input, and a transformed AST can print longer than its source.
#[must_use]
pub fn compact_with_capacity(block: &Block, capacity: usize) -> String {
    let mut printer =
        CompactPrinter { output: CodeBuffer::with_capacity(capacity), previous: PieceClass::Other };
    printer.emit_block(block);
    printer.output.into_string()
}

struct CompactPrinter {
    output: CodeBuffer,
    previous: PieceClass,
}

impl CompactPrinter {
    /// Print one piece, preceded by a space if it would otherwise merge with
    /// the previous one.
    #[expect(
        clippy::inline_always,
        reason = "keeps fixed spellings constant through the spacing check and buffer write"
    )]
    #[inline(always)]
    fn emit_piece(&mut self, text: &str, class: PieceClass) {
        let Some(&first_byte) = text.as_bytes().first() else {
            return;
        };
        if needs_space(self.previous, first_byte, class) {
            self.output.print_ascii_byte(b' ');
        }
        self.output.print_str(text);
        self.previous = class;
    }

    /// Emit a fixed-spelling piece (keyword, operator, punctuation).
    #[inline]
    fn emit_fixed(&mut self, text: &'static str) {
        self.emit_piece(text, PieceClass::of_fixed(text));
    }

    /// Emit a payload-carrying token (identifier, number, string literal).
    /// Fixed-spelling kinds (`Type::Singleton` carries `nil`/`true`/`false`)
    /// fall through to their static text.
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "any other token takes the default path, and naming the rest of TokenKind adds nothing"
    )]
    fn emit_token(&mut self, token: &Token) {
        match &token.kind {
            TokenKind::Identifier(text) => self.emit_piece(text, PieceClass::Word),
            TokenKind::Number(text) => self.emit_piece(text, PieceClass::Number),
            TokenKind::StringLiteral(text) => self.emit_piece(text, PieceClass::Other),
            TokenKind::Nil => self.emit_fixed("nil"),
            TokenKind::True => self.emit_fixed("true"),
            TokenKind::False => self.emit_fixed("false"),
            other => {
                debug_assert!(false, "unexpected token kind in AST: {other:?}");
            }
        }
    }

    /// Emit `items` with `separator` between them and none after the last.
    fn emit_joined<T>(
        &mut self,
        items: &[T],
        separator: &'static str,
        mut emit_item: impl FnMut(&mut Self, &T),
    ) {
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.emit_fixed(separator);
            }
            emit_item(self, item);
        }
    }

    /// Emit `punctuated` with `separator` between its items, keeping a trailing
    /// separator the source carried. The owning node knows the spelling,
    /// which `Punctuated` doesn't store.
    fn emit_punctuated<T>(
        &mut self,
        punctuated: &Punctuated<T>,
        separator: &'static str,
        emit_item: impl FnMut(&mut Self, &T),
    ) {
        self.emit_joined(&punctuated.items, separator, emit_item);
        if punctuated.has_trailing_separator && !punctuated.is_empty() {
            self.emit_fixed(separator);
        }
    }

    fn emit_block(&mut self, block: &Block) {
        for (index, stmt) in block.stmts.iter().enumerate() {
            // Without a `;`, a leading `(` would re-parse as a call on whatever
            // expression ends the previous statement.
            if index > 0 && stmt_starts_with_paren(stmt) && self.previous != PieceClass::Semicolon {
                self.emit_fixed(";");
            }
            self.emit_statement(stmt);
        }
        if let Some(last) = &block.last_stmt {
            self.emit_last_statement(last);
        }
    }

    fn emit_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Assignment(assignment) => self.emit_assignment(assignment),
            Statement::FunctionCall(call) => self.emit_function_call(call),
            Statement::DoBlock(block) => self.emit_do_block(block),
            Statement::WhileLoop(while_loop) => self.emit_while_loop(while_loop),
            Statement::RepeatLoop(repeat_loop) => self.emit_repeat_loop(repeat_loop),
            Statement::IfStatement(if_stmt) => self.emit_if_statement(if_stmt),
            Statement::NumericFor(numeric_for) => self.emit_numeric_for(numeric_for),
            Statement::GenericFor(generic_for) => self.emit_generic_for(generic_for),
            Statement::FunctionDecl(declaration) => self.emit_function_declaration(declaration),
            Statement::LocalFunction(local_function) => self.emit_local_function(local_function),
            Statement::LocalAssignment(assignment) => self.emit_local_assignment(assignment),
            Statement::EmptyStatement(_) => self.emit_fixed(";"),
            // Lua 5.2+
            Statement::Goto(goto_stmt) => self.emit_goto(goto_stmt),
            // Lua 5.2+
            Statement::Label(label) => self.emit_label(label),
            // Lua 5.5
            Statement::GlobalDeclaration(declaration) => self.emit_global_declaration(declaration),
            // Lua 5.5
            Statement::GlobalFunction(function) => self.emit_global_function(function),
            // Lua 5.5
            Statement::GlobalStar(global_star) => self.emit_global_star(global_star),
            // Lua 5.2+
            Statement::Break(_) => self.emit_fixed("break"),
            // Luau
            Statement::CompoundAssignment(compound) => self.emit_compound_assignment(compound),
            // Luau
            Statement::TypeDeclaration(declaration) => self.emit_type_declaration(declaration),
            Statement::Error(_) => {}
        }
    }

    fn emit_last_statement(&mut self, stmt: &LastStatement) {
        match stmt {
            LastStatement::Return(return_stmt) => self.emit_return(return_stmt),
            LastStatement::Break(_) => self.emit_fixed("break"),
            // Luau
            LastStatement::Continue(_) => self.emit_fixed("continue"),
            LastStatement::Error(_) => {}
        }
    }

    fn emit_assignment(&mut self, assignment: &Assignment) {
        self.emit_punctuated(&assignment.targets, ",", Self::emit_var);
        self.emit_fixed("=");
        self.emit_punctuated(&assignment.values, ",", Self::emit_expression);
    }

    fn emit_do_block(&mut self, block: &DoBlock) {
        self.emit_fixed("do");
        self.emit_block(&block.block);
        self.emit_fixed("end");
    }

    fn emit_while_loop(&mut self, while_loop: &WhileLoop) {
        self.emit_fixed("while");
        self.emit_expression(&while_loop.condition);
        self.emit_fixed("do");
        self.emit_block(&while_loop.block);
        self.emit_fixed("end");
    }

    fn emit_repeat_loop(&mut self, repeat_loop: &RepeatLoop) {
        self.emit_fixed("repeat");
        self.emit_block(&repeat_loop.block);
        self.emit_fixed("until");
        self.emit_expression(&repeat_loop.condition);
    }

    fn emit_if_statement(&mut self, if_stmt: &IfStatement) {
        self.emit_fixed("if");
        self.emit_expression(&if_stmt.condition);
        self.emit_fixed("then");
        self.emit_block(&if_stmt.block);
        for clause in &if_stmt.elseif_clauses {
            self.emit_fixed("elseif");
            self.emit_expression(&clause.condition);
            self.emit_fixed("then");
            self.emit_block(&clause.block);
        }
        if let Some(else_clause) = &if_stmt.else_clause {
            self.emit_fixed("else");
            self.emit_block(&else_clause.block);
        }
        self.emit_fixed("end");
    }

    fn emit_numeric_for(&mut self, numeric_for: &NumericFor) {
        self.emit_fixed("for");
        self.emit_token(&numeric_for.name);
        if let Some(type_value) = &numeric_for.type_annotation {
            self.emit_fixed(":");
            self.emit_type(type_value);
        }
        self.emit_fixed("=");
        self.emit_expression(&numeric_for.start);
        self.emit_fixed(",");
        self.emit_expression(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.emit_fixed(",");
            self.emit_expression(step);
        }
        self.emit_fixed("do");
        self.emit_block(&numeric_for.block);
        self.emit_fixed("end");
    }

    fn emit_generic_for(&mut self, generic_for: &GenericFor) {
        self.emit_fixed("for");
        self.emit_punctuated(&generic_for.names, ",", Self::emit_param);
        self.emit_fixed("in");
        self.emit_punctuated(&generic_for.exprs, ",", Self::emit_expression);
        self.emit_fixed("do");
        self.emit_block(&generic_for.block);
        self.emit_fixed("end");
    }

    fn emit_function_declaration(&mut self, function_declaration: &FunctionDecl) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        self.emit_function_attributes(&function_declaration.attributes);
        self.emit_fixed("function");
        self.emit_function_name(&function_declaration.name);
        self.emit_function_body(&function_declaration.body);
    }

    fn emit_function_name(&mut self, name: &FunctionName) {
        self.emit_joined(&name.names, ".", Self::emit_token);
        if let Some(method_name) = &name.method {
            self.emit_fixed(":");
            self.emit_token(method_name);
        }
    }

    fn emit_local_function(&mut self, local_function: &LocalFunction) {
        // Luau: `@native` etc. change runtime behavior - never drop them.
        self.emit_function_attributes(&local_function.attributes);
        if local_function.is_exported {
            self.emit_fixed("export");
        } else {
            self.emit_fixed(if local_function.is_const { "const" } else { "local" });
        }
        self.emit_fixed("function");
        self.emit_token(&local_function.name);
        self.emit_function_body(&local_function.body);
    }

    /// Emit the Luau attribute list. Arguments only exist on the
    /// bracketed form, so attributes with args re-emit as `@[name(...)]`.
    fn emit_function_attributes(&mut self, attributes: &[FunctionAttribute]) {
        for attribute in attributes {
            self.emit_fixed("@");
            if let Some(args) = &attribute.args {
                self.emit_fixed("[");
                self.emit_token(&attribute.name);
                self.emit_fixed("(");
                self.emit_punctuated(args, ",", Self::emit_expression);
                self.emit_fixed(")");
                self.emit_fixed("]");
            } else {
                self.emit_token(&attribute.name);
            }
        }
    }

    fn emit_local_assignment(&mut self, local_assignment: &LocalAssignment) {
        if local_assignment.is_exported {
            self.emit_fixed("export");
        }
        self.emit_fixed(if local_assignment.is_const { "const" } else { "local" });
        self.emit_punctuated(&local_assignment.names, ",", Self::emit_attributed_name);
        if let Some(exprs) = &local_assignment.exprs {
            self.emit_fixed("=");
            self.emit_punctuated(exprs, ",", Self::emit_expression);
        }
    }

    fn emit_attributed_name(&mut self, attributed: &AttributedName) {
        self.emit_token(&attributed.name);
        if let Some(type_value) = &attributed.type_annotation {
            self.emit_fixed(":");
            self.emit_type(type_value);
        }
        if let Some(attribute) = &attributed.attribute {
            self.emit_fixed("<");
            self.emit_token(&attribute.name);
            self.emit_fixed(">");
        }
    }

    fn emit_goto(&mut self, goto_stmt: &GotoStatement) {
        self.emit_fixed("goto");
        self.emit_token(&goto_stmt.name);
    }

    fn emit_label(&mut self, label: &LabelStatement) {
        self.emit_fixed("::");
        self.emit_token(&label.name);
        self.emit_fixed("::");
    }

    fn emit_return(&mut self, return_stmt: &ReturnStatement) {
        self.emit_fixed("return");
        self.emit_punctuated(&return_stmt.exprs, ",", Self::emit_expression);
    }

    fn emit_compound_assignment(&mut self, compound: &CompoundAssignment) {
        self.emit_var(&compound.var);
        self.emit_fixed(compound.op.static_text());
        self.emit_expression(&compound.expr);
    }

    fn emit_type_declaration(&mut self, type_declaration: &TypeDeclaration) {
        if type_declaration.is_exported {
            self.emit_fixed("export");
        }
        self.emit_fixed("type");
        // Luau `type function Name funcbody` - no `=`.
        if matches!(type_declaration.type_value, TypeDeclarationValue::TypeFunction(_)) {
            self.emit_fixed("function");
        }
        self.emit_token(&type_declaration.name);
        if let Some(generics) = &type_declaration.generics {
            self.emit_generic_type_list(generics);
        }
        match &type_declaration.type_value {
            TypeDeclarationValue::Alias(type_value) => {
                self.emit_fixed("=");
                self.emit_type(type_value);
            }
            TypeDeclarationValue::TypeFunction(body) => self.emit_function_body(body),
        }
    }

    fn emit_global_declaration(&mut self, global_declaration: &GlobalDeclaration) {
        self.emit_fixed("global");
        self.emit_punctuated(&global_declaration.names, ",", Self::emit_attributed_name);
        if let Some(exprs) = &global_declaration.exprs {
            self.emit_fixed("=");
            self.emit_punctuated(exprs, ",", Self::emit_expression);
        }
    }

    fn emit_global_function(&mut self, global_function: &GlobalFunction) {
        self.emit_fixed("global");
        self.emit_fixed("function");
        self.emit_token(&global_function.name);
        self.emit_function_body(&global_function.body);
    }

    fn emit_global_star(&mut self, global_star: &GlobalStar) {
        self.emit_fixed("global");
        if let Some(attribute) = &global_star.attribute {
            self.emit_fixed("<");
            self.emit_token(&attribute.name);
            self.emit_fixed(">");
        }
        self.emit_fixed("*");
    }

    fn emit_expression(&mut self, expr: &Expression) {
        match expr {
            Expression::Nil(_) => self.emit_fixed("nil"),
            Expression::False(_) => self.emit_fixed("false"),
            Expression::True(_) => self.emit_fixed("true"),
            Expression::VarArg(_) => self.emit_fixed("..."),
            Expression::Number(literal) | Expression::Integer(literal) => {
                self.emit_piece(&literal.text, PieceClass::Number);
            }
            Expression::StringLiteral(literal) => {
                self.emit_piece(&literal.text, PieceClass::Other);
            }
            Expression::FunctionDef(function) => self.emit_function_definition(function),
            Expression::Var(var) => self.emit_var(var),
            Expression::FunctionCall(call) => self.emit_function_call(call),
            Expression::Parenthesized(parenthesized) => {
                self.emit_parenthesized_expression(parenthesized);
            }
            Expression::TableConstructor(table) => self.emit_table_constructor(table),
            Expression::BinaryOp(binary_op) => self.emit_binary_op(binary_op),
            Expression::UnaryOp(unary_op) => self.emit_unary_op(unary_op),
            // Luau
            Expression::IfExpression(if_expr) => self.emit_if_expression(if_expr),
            // Luau
            Expression::InterpolatedString(string) => self.emit_interpolated_string(string),
            // Luau
            Expression::TypeCast(cast) => self.emit_type_cast(cast),
            // Luau
            Expression::TypeInstantiation(instantiation) => {
                self.emit_type_instantiation(instantiation);
            }
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

    fn emit_function_definition(&mut self, function_definition: &FunctionDef) {
        self.emit_function_attributes(&function_definition.attributes);
        self.emit_fixed("function");
        self.emit_function_body(&function_definition.body);
    }

    fn emit_function_call(&mut self, call: &FunctionCall) {
        self.emit_expression(&call.callee);
        if let Some(method_name) = &call.method {
            self.emit_fixed(":");
            self.emit_token(method_name);
        }
        if let Some(type_args) = &call.explicit_type_args {
            self.emit_fixed("<<");
            self.emit_punctuated(&type_args.args, ",", Self::emit_type);
            self.emit_fixed(">>");
        }
        self.emit_function_args(&call.args);
    }

    fn emit_function_args(&mut self, function_args: &FunctionArgs) {
        match function_args {
            FunctionArgs::Parenthesized { args, .. } => {
                self.emit_fixed("(");
                self.emit_punctuated(args, ",", Self::emit_expression);
                self.emit_fixed(")");
            }
            FunctionArgs::TableConstructor(table) => self.emit_table_constructor(table),
            FunctionArgs::StringLiteral(literal) => {
                self.emit_piece(&literal.text, PieceClass::Other);
            }
        }
    }

    fn emit_parenthesized_expression(&mut self, parenthesized: &ParenthesizedExpression) {
        self.emit_fixed("(");
        self.emit_expression(&parenthesized.expr);
        self.emit_fixed(")");
    }

    fn emit_table_constructor(&mut self, table: &TableConstructor) {
        self.emit_fixed("{");
        // A trailing separator the source carried is dropped; it costs bytes
        // and changes nothing.
        self.emit_joined(&table.fields.items, ",", Self::emit_field);
        self.emit_fixed("}");
    }

    fn emit_field(&mut self, field: &Field) {
        match field {
            Field::Bracketed { key, value, .. } => {
                self.emit_fixed("[");
                self.emit_expression(key);
                self.emit_fixed("]");
                self.emit_fixed("=");
                self.emit_expression(value);
            }
            Field::Named { name, value, .. } => {
                self.emit_token(name);
                self.emit_fixed("=");
                self.emit_expression(value);
            }
            Field::Positional { value, .. } => {
                self.emit_expression(value);
            }
        }
    }

    fn emit_index_expression(&mut self, index: &IndexExpression) {
        self.emit_expression(&index.prefix);
        self.emit_fixed("[");
        self.emit_expression(&index.index);
        self.emit_fixed("]");
    }

    fn emit_field_access(&mut self, field: &FieldAccess) {
        self.emit_expression(&field.prefix);
        self.emit_fixed(".");
        self.emit_token(&field.name);
    }

    fn emit_binary_op(&mut self, binary_op: &BinaryOp) {
        self.emit_expression(&binary_op.left);
        self.emit_fixed(binary_op.op.static_text());
        self.emit_expression(&binary_op.right);
    }

    fn emit_unary_op(&mut self, unary_op: &UnaryOp) {
        self.emit_fixed(unary_op.op.static_text());
        self.emit_expression(&unary_op.operand);
    }

    fn emit_if_expression(&mut self, if_expr: &IfExpression) {
        self.emit_fixed("if");
        self.emit_expression(&if_expr.condition);
        self.emit_fixed("then");
        self.emit_expression(&if_expr.then_expr);
        for clause in &if_expr.elseif_clauses {
            self.emit_fixed("elseif");
            self.emit_expression(&clause.condition);
            self.emit_fixed("then");
            self.emit_expression(&clause.expr);
        }
        self.emit_fixed("else");
        self.emit_expression(&if_expr.else_expr);
    }

    fn emit_interpolated_string(&mut self, interpolated: &InterpolatedString) {
        // The `` ` ``/`{`/`}` punctuation follows expression presence, not
        // token kind: a plain `` `text` `` lexes as a Begin("") + End(text)
        // pair with no expression and must print without braces.
        let mut has_previous_expr = false;
        for (index, segment) in interpolated.segments.iter().enumerate() {
            if index == 0 {
                self.output.print_ascii_byte(b'`');
            }
            if has_previous_expr {
                self.output.print_ascii_byte(b'}');
            }
            if let TokenKind::InterpolatedStringBegin(text)
            | TokenKind::InterpolatedStringMiddle(text)
            | TokenKind::InterpolatedStringEnd(text) = &segment.literal.kind
            {
                self.output.print_str(text);
            }
            self.previous = PieceClass::Other;
            if let Some(expr) = &segment.expr {
                self.output.print_ascii_byte(b'{');
                // `{{` is a parse error in Luau: an expression starting
                // with a table constructor needs a space after the
                // interpolation opener.
                if expr_starts_with_brace(expr) {
                    self.output.print_ascii_byte(b' ');
                }
                self.emit_expression(expr);
            }
            has_previous_expr = segment.expr.is_some();
        }
        self.output.print_ascii_byte(b'`');
        self.previous = PieceClass::Other;
    }

    fn emit_type_cast(&mut self, cast: &TypeCast) {
        self.emit_expression(&cast.expr);
        self.emit_fixed("::");
        self.emit_type(&cast.type_annotation);
    }

    fn emit_type_instantiation(&mut self, instantiation: &TypeInstantiation) {
        self.emit_expression(&instantiation.expr);
        self.emit_fixed("<<");
        self.emit_punctuated(&instantiation.type_args.args, ",", Self::emit_type);
        self.emit_fixed(">>");
    }

    fn emit_function_body(&mut self, body: &FunctionBody) {
        // Luau: `<T, U...>` generics sit between the function name and `(`.
        if let Some(generics) = &body.generics {
            self.emit_generic_type_list(generics);
        }
        self.emit_fixed("(");
        self.emit_joined(&body.params.items, ",", Self::emit_param);
        if let Some(vararg) = &body.vararg {
            // The AST doesn't store the comma between the last param and `...`.
            if !body.params.is_empty() {
                self.emit_fixed(",");
            }
            self.emit_fixed("...");
            if let Some(name) = &vararg.name {
                self.emit_token(name);
            }
            if let Some(type_value) = &vararg.type_annotation {
                self.emit_fixed(":");
                self.emit_type(type_value);
            }
        }
        self.emit_fixed(")");
        if let Some(return_type) = &body.return_type {
            self.emit_fixed(":");
            self.emit_type(return_type);
        }
        self.emit_block(&body.block);
        self.emit_fixed("end");
    }

    fn emit_param(&mut self, param: &Parameter) {
        self.emit_token(&param.name);
        if let Some(type_value) = &param.type_annotation {
            self.emit_fixed(":");
            self.emit_type(type_value);
        }
    }

    fn emit_type(&mut self, ty: &Type) {
        match ty {
            Type::Name { name, .. } => self.emit_piece(name, PieceClass::Word),
            Type::Named(named) => self.emit_named_type(named),
            Type::Typeof(typeof_type) => self.emit_typeof_type(typeof_type),
            Type::Table(table) => self.emit_table_type(table),
            Type::Function(function) => self.emit_function_type(function),
            Type::Optional(optional) => self.emit_optional_type(optional),
            Type::Union(union) => self.emit_union_type(union),
            Type::Intersection(intersection) => self.emit_intersection_type(intersection),
            Type::Parenthesized(parenthesized) => self.emit_parenthesized_type(parenthesized),
            Type::Pack(pack) => self.emit_type_pack(pack),
            Type::Singleton(token) => self.emit_token(token),
            Type::Variadic(variadic) => self.emit_variadic_type(variadic),
            Type::GenericPack(generic_pack) => self.emit_generic_pack_type(generic_pack),
            // Mirrors `Statement::Error` and `Expression::Error`, which also
            // emit nothing.
            Type::Error(_) => {}
        }
    }

    fn emit_named_type(&mut self, named: &NamedType) {
        if let Some(module) = &named.prefix {
            self.emit_token(module);
            self.emit_fixed(".");
        }
        self.emit_token(&named.name);
        if let Some(generics) = &named.generics {
            self.emit_type_args(generics);
        }
    }

    fn emit_type_args(&mut self, args: &TypeArgs) {
        self.emit_fixed("<");
        self.emit_punctuated(&args.args, ",", Self::emit_type);
        self.emit_fixed(">");
    }

    fn emit_typeof_type(&mut self, typeof_type: &TypeofType) {
        self.emit_fixed("typeof");
        self.emit_fixed("(");
        self.emit_expression(&typeof_type.expr);
        self.emit_fixed(")");
    }

    fn emit_table_type(&mut self, table: &TableType) {
        self.emit_fixed("{");
        // Trailing separators are dropped, as in table constructors.
        self.emit_joined(&table.fields.items, ",", Self::emit_type_field);
        self.emit_fixed("}");
    }

    fn emit_type_field(&mut self, field: &TypeField) {
        match field {
            TypeField::Named { access, name, value, .. } => {
                // Luau `read`/`write` is a word, so the separator spaces it
                // from the name.
                if let Some(access_token) = access {
                    self.emit_token(access_token);
                }
                self.emit_token(name);
                self.emit_fixed(":");
                self.emit_type(value);
            }
            TypeField::Indexer { access, key, value, .. } => {
                if let Some(access_token) = access {
                    self.emit_token(access_token);
                }
                self.emit_fixed("[");
                self.emit_type(key);
                self.emit_fixed("]");
                self.emit_fixed(":");
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
        self.emit_fixed("(");
        self.emit_punctuated(&function.params, ",", Self::emit_function_type_param);
        self.emit_fixed(")");
        self.emit_fixed("->");
        self.emit_type(&function.return_type);
    }

    fn emit_function_type_param(&mut self, param: &FunctionTypeParam) {
        if let Some(name) = &param.name {
            self.emit_token(name);
            self.emit_fixed(":");
        }
        self.emit_type(&param.type_value);
    }

    fn emit_optional_type(&mut self, optional: &OptionalType) {
        self.emit_type(&optional.type_value);
        self.emit_fixed("?");
    }

    fn emit_union_type(&mut self, union: &UnionType) {
        if union.has_leading_pipe {
            self.emit_fixed("|");
        }
        self.emit_punctuated(&union.types, "|", Self::emit_type);
    }

    fn emit_intersection_type(&mut self, intersection: &IntersectionType) {
        if intersection.has_leading_ampersand {
            self.emit_fixed("&");
        }
        self.emit_punctuated(&intersection.types, "&", Self::emit_type);
    }

    fn emit_parenthesized_type(&mut self, parenthesized: &ParenthesizedType) {
        self.emit_fixed("(");
        self.emit_type(&parenthesized.type_value);
        self.emit_fixed(")");
    }

    fn emit_type_pack(&mut self, pack: &TypePack) {
        self.emit_fixed("(");
        self.emit_punctuated(&pack.types, ",", Self::emit_type);
        self.emit_fixed(")");
    }

    fn emit_variadic_type(&mut self, variadic: &VariadicType) {
        self.emit_fixed("...");
        self.emit_type(&variadic.type_value);
    }

    fn emit_generic_pack_type(&mut self, generic_pack: &GenericPackType) {
        self.emit_token(&generic_pack.name);
        self.emit_fixed("...");
    }

    fn emit_generic_type_list(&mut self, generics: &GenericTypeList) {
        self.emit_fixed("<");
        self.emit_punctuated(&generics.params, ",", Self::emit_generic_type_param);
        self.emit_fixed(">");
    }

    fn emit_generic_type_param(&mut self, param: &GenericTypeParam) {
        self.emit_token(&param.name);
        if param.is_pack {
            self.emit_fixed("...");
        }
        if let Some(default) = &param.default {
            self.emit_fixed("=");
            self.emit_type(default);
        }
    }
}
