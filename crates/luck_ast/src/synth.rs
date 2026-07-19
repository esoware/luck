//! Programmatic AST synthesis.
//!
//! Builds `luck_ast` nodes without any source text - the intended consumer is
//! a Luau bytecode decompiler that emits an AST directly. Hand-writing the node
//! structs means spelling out every `Token { kind, span }`; this module hides
//! that behind small constructors.
//!
//! Every synthesized node gets a fresh single-point span `Span::new(n, n)` with
//! `n` strictly increasing. The spans do not index into any source - they exist
//! only to keep nodes distinguishable for comment anchoring and diagnostics.

use luck_token::{CompactString, Span, Token, TokenKind};

use crate::expr::*;
use crate::shared::*;
use crate::stmt::*;
use crate::types::*;

/// Monotonic span allocator + node constructors.
#[derive(Debug, Default)]
pub struct Synth {
    next_offset: u32,
}

/// A table field passed to [`Synth::table`], before the separators and spans
/// the real [`Field`] carries are filled in.
pub enum SynthField {
    Positional(Expression),
    Named(String, Expression),
    Bracketed(Expression, Expression),
}

/// A comment to attach during a later formatting pass. Comments are not in the
/// AST (they live in a side table keyed by `attached_to`), so synthesis returns
/// this as dumb data for the formatter to consume; nothing here interprets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntheticComment {
    pub attached_to: u32,
    pub text: CompactString,
    pub is_leading: bool,
}

impl Synth {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh single-point span. Distinct per node; never slices source.
    fn next_span(&mut self) -> Span {
        let offset = self.next_offset;
        self.next_offset += 1;
        Span::new(offset, offset)
    }

    pub fn token(&mut self, kind: TokenKind) -> Token {
        Token::new(kind, self.next_span())
    }

    pub fn ident(&mut self, name: &str) -> Token {
        self.token(TokenKind::Identifier(CompactString::from(name)))
    }

    pub fn name_expr(&mut self, name: &str) -> Expression {
        let name = self.ident(name);
        Expression::Var(Box::new(Var::Name(name)))
    }

    pub fn number(&mut self, text: &str) -> Expression {
        let token = self.token(TokenKind::Number(CompactString::from(text)));
        Expression::Number(token)
    }

    /// String literal from UNQUOTED content; the stored token text is the raw
    /// double-quoted form.
    pub fn string(&mut self, content: &str) -> Expression {
        let token = self.token(TokenKind::StringLiteral(raw_string(content)));
        Expression::StringLiteral(token)
    }

    pub fn nil(&mut self) -> Expression {
        let token = self.token(TokenKind::Nil);
        Expression::Nil(token)
    }

    pub fn bool(&mut self, value: bool) -> Expression {
        if value {
            let token = self.token(TokenKind::True);
            Expression::True(token)
        } else {
            let token = self.token(TokenKind::False);
            Expression::False(token)
        }
    }

    pub fn vararg(&mut self) -> Expression {
        let token = self.token(TokenKind::DotDotDot);
        Expression::VarArg(token)
    }

    pub fn binop(&mut self, lhs: Expression, op: TokenKind, rhs: Expression) -> Expression {
        Expression::BinaryOp(Box::new(BinaryOp {
            span: self.next_span(),
            left: lhs,
            op: self.token(op),
            right: rhs,
        }))
    }

    pub fn unop(&mut self, op: TokenKind, operand: Expression) -> Expression {
        Expression::UnaryOp(Box::new(UnaryOp {
            span: self.next_span(),
            op: self.token(op),
            operand,
        }))
    }

    pub fn paren(&mut self, expr: Expression) -> Expression {
        Expression::Parenthesized(Box::new(ParenExpression {
            span: self.next_span(),
            parens: self.contained(TokenKind::LeftParen, TokenKind::RightParen),
            expr,
        }))
    }

    pub fn index(&mut self, prefix: Expression, index: Expression) -> Expression {
        Expression::Var(Box::new(Var::Index(Box::new(IndexExpression {
            span: self.next_span(),
            prefix,
            brackets: self.contained(TokenKind::LeftBracket, TokenKind::RightBracket),
            index,
        }))))
    }

    pub fn field(&mut self, prefix: Expression, name: &str) -> Expression {
        Expression::Var(Box::new(Var::FieldAccess(Box::new(FieldAccess {
            span: self.next_span(),
            prefix,
            dot: self.token(TokenKind::Dot),
            name: self.ident(name),
        }))))
    }

    pub fn call(&mut self, callee: Expression, args: Vec<Expression>) -> Expression {
        Expression::FunctionCall(Box::new(FunctionCall {
            span: self.next_span(),
            callee,
            args: self.paren_args(args),
            method: None,
        }))
    }

    pub fn method_call(
        &mut self,
        receiver: Expression,
        name: &str,
        args: Vec<Expression>,
    ) -> Expression {
        Expression::FunctionCall(Box::new(FunctionCall {
            span: self.next_span(),
            callee: receiver,
            args: self.paren_args(args),
            method: Some((self.token(TokenKind::Colon), self.ident(name))),
        }))
    }

    pub fn table(&mut self, fields: Vec<SynthField>) -> Expression {
        let span = self.next_span();
        let braces = self.contained(TokenKind::LeftBrace, TokenKind::RightBrace);
        let count = fields.len();
        let mut built = Vec::with_capacity(count);
        for (index, field) in fields.into_iter().enumerate() {
            let node = match field {
                SynthField::Positional(value) => Field::Positional {
                    span: self.next_span(),
                    value,
                },
                SynthField::Named(name, value) => Field::Named {
                    span: self.next_span(),
                    name: self.ident(&name),
                    equal: self.token(TokenKind::Equal),
                    value,
                },
                SynthField::Bracketed(key, value) => Field::Bracketed {
                    span: self.next_span(),
                    brackets: self.contained(TokenKind::LeftBracket, TokenKind::RightBracket),
                    key,
                    equal: self.token(TokenKind::Equal),
                    value,
                },
            };
            // The separator FOLLOWS its item; the final field carries none.
            let separator = if index + 1 < count {
                Some(self.token(TokenKind::Comma))
            } else {
                None
            };
            built.push((node, separator));
        }
        Expression::TableConstructor(Box::new(TableConstructor {
            span,
            braces,
            fields: built,
        }))
    }

    pub fn function_def(&mut self, params: Vec<&str>, body: Block) -> Expression {
        let params = self.params(params);
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            function_token: self.token(TokenKind::Function),
            body: self.function_body(params, None, body),
        }))
    }

    pub fn function_def_typed(
        &mut self,
        params: Vec<Parameter>,
        return_type: Option<Type>,
        body: Block,
    ) -> Expression {
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            function_token: self.token(TokenKind::Function),
            body: self.function_body(params, return_type, body),
        }))
    }

    /// A function whose parameter list ends with `...` (`FunctionBody.vararg`).
    /// The trailing `...` may carry a Luau pack annotation (`...: number`).
    pub fn function_def_variadic(
        &mut self,
        params: Vec<Parameter>,
        vararg_type: Option<Type>,
        return_type: Option<Type>,
        body: Block,
    ) -> Expression {
        let mut function_body = self.function_body(params, return_type, body);
        function_body.vararg = Some(self.vararg_param(vararg_type));
        Expression::FunctionDef(Box::new(FunctionDef {
            span: self.next_span(),
            function_token: self.token(TokenKind::Function),
            body: function_body,
        }))
    }

    /// Luau type cast: `expr :: T`.
    pub fn type_cast(&mut self, expr: Expression, type_value: Type) -> Expression {
        Expression::TypeCast(Box::new(TypeCast {
            span: self.next_span(),
            expr,
            double_colon: self.token(TokenKind::DoubleColon),
            type_annotation: type_value,
        }))
    }

    /// Luau backtick string. Each tuple is one segment: leading literal text
    /// plus an optional interpolated expression. The first segment's text is an
    /// `InterpBegin`, the last an `InterpEnd`, the rest `InterpMid` - the exact
    /// shape the parser builds, where the terminal segment holds the trailing
    /// text with no expression (a plain `` `text` `` is
    /// `[("", None), ("text", None)]`, mirroring the lexer's `InterpBegin("")` +
    /// `InterpEnd(text)` pair).
    pub fn interpolated_string(
        &mut self,
        segments: Vec<(String, Option<Expression>)>,
    ) -> Expression {
        let span = self.next_span();
        let last_index = segments.len().saturating_sub(1);
        let built: Vec<InterpSegment> = segments
            .into_iter()
            .enumerate()
            .map(|(index, (text, expr))| {
                let literal = if index == 0 {
                    self.token(TokenKind::InterpBegin(CompactString::from(text)))
                } else if index == last_index {
                    self.token(TokenKind::InterpEnd(CompactString::from(text)))
                } else {
                    self.token(TokenKind::InterpMid(CompactString::from(text)))
                };
                InterpSegment { literal, expr }
            })
            .collect();
        Expression::InterpolatedString(Box::new(InterpolatedString {
            span,
            segments: built,
        }))
    }

    pub fn if_expr(
        &mut self,
        cond: Expression,
        then_expr: Expression,
        else_expr: Expression,
    ) -> Expression {
        Expression::IfExpression(Box::new(IfExpression {
            span: self.next_span(),
            if_token: self.token(TokenKind::If),
            condition: cond,
            then_token: self.token(TokenKind::Then),
            then_expr,
            elseif_clauses: Vec::new(),
            else_token: self.token(TokenKind::Else),
            else_expr,
        }))
    }

    pub fn local(&mut self, names: Vec<&str>, exprs: Vec<Expression>) -> Statement {
        let typed = names.into_iter().map(|name| (name, None)).collect();
        self.local_typed(typed, exprs)
    }

    pub fn local_typed(
        &mut self,
        names: Vec<(&str, Option<Type>)>,
        exprs: Vec<Expression>,
    ) -> Statement {
        let span = self.next_span();
        let local_token = self.token(TokenKind::Local);
        let attributed: Vec<AttributedName> = names
            .into_iter()
            .map(|(name, ty)| AttributedName {
                name: self.ident(name),
                type_annotation: ty.map(|ty| (self.token(TokenKind::Colon), ty)),
                attrib: None,
            })
            .collect();
        let names = self.punctuated(attributed);
        let equal_and_exprs = if exprs.is_empty() {
            None
        } else {
            Some((self.token(TokenKind::Equal), self.punctuated(exprs)))
        };
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            local_token,
            names,
            equal_and_exprs,
        }))
    }

    /// Lua 5.4 local with per-name attributes: `local x <const>, y <close> = ...`.
    /// Each name carries an optional attribute keyword (`"const"` / `"close"`).
    pub fn attributed_local(
        &mut self,
        names: Vec<(&str, Option<&str>)>,
        exprs: Vec<Expression>,
    ) -> Statement {
        let span = self.next_span();
        let local_token = self.token(TokenKind::Local);
        let attributed: Vec<AttributedName> = names
            .into_iter()
            .map(|(name, attribute)| AttributedName {
                name: self.ident(name),
                type_annotation: None,
                attrib: attribute.map(|attribute| Attribute {
                    span: self.next_span(),
                    open: self.token(TokenKind::Less),
                    name: self.ident(attribute),
                    close: self.token(TokenKind::Greater),
                }),
            })
            .collect();
        let names = self.punctuated(attributed);
        let equal_and_exprs = if exprs.is_empty() {
            None
        } else {
            Some((self.token(TokenKind::Equal), self.punctuated(exprs)))
        };
        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            local_token,
            names,
            equal_and_exprs,
        }))
    }

    pub fn assign(&mut self, targets: Vec<Expression>, values: Vec<Expression>) -> Statement {
        let vars: Vec<Var> = targets.into_iter().map(Self::expr_to_var).collect();
        Statement::Assignment(Box::new(Assignment {
            span: self.next_span(),
            targets: self.punctuated(vars),
            equal: self.token(TokenKind::Equal),
            values: self.punctuated(values),
        }))
    }

    /// Luau compound assignment: `target op= value`. Accepts only the compound
    /// operator token kinds; any other kind is a synthesis-side programmer error.
    pub fn compound_assign(
        &mut self,
        target: Expression,
        op: TokenKind,
        value: Expression,
    ) -> Statement {
        // Guard the token kind up front: a stray operator would emit a node the
        // parser could never have produced.
        match op {
            TokenKind::PlusEqual
            | TokenKind::MinusEqual
            | TokenKind::StarEqual
            | TokenKind::SlashEqual
            | TokenKind::FloorDivEqual
            | TokenKind::PercentEqual
            | TokenKind::CaretEqual
            | TokenKind::DotDotEqual => {}
            other => panic!(
                "synth: compound_assign requires a compound-assignment operator, got {other:?}"
            ),
        }
        Statement::CompoundAssignment(Box::new(CompoundAssignment {
            span: self.next_span(),
            var: Self::expr_to_var(target),
            op: self.token(op),
            expr: value,
        }))
    }

    pub fn call_stmt(&mut self, call_expr: Expression) -> Statement {
        let call = match call_expr {
            Expression::FunctionCall(call) => *call,
            other => panic!("synth: call_stmt requires a function-call expression, got {other:?}"),
        };
        Statement::FunctionCall(Box::new(FunctionCallStmt {
            span: self.next_span(),
            call,
        }))
    }

    pub fn do_block(&mut self, block: Block) -> Statement {
        Statement::DoBlock(Box::new(DoBlock {
            span: self.next_span(),
            do_token: self.token(TokenKind::Do),
            block,
            end_token: self.token(TokenKind::End),
        }))
    }

    pub fn while_(&mut self, cond: Expression, block: Block) -> Statement {
        Statement::WhileLoop(Box::new(WhileLoop {
            span: self.next_span(),
            while_token: self.token(TokenKind::While),
            condition: cond,
            do_token: self.token(TokenKind::Do),
            block,
            end_token: self.token(TokenKind::End),
        }))
    }

    pub fn repeat_(&mut self, block: Block, cond: Expression) -> Statement {
        Statement::RepeatLoop(Box::new(RepeatLoop {
            span: self.next_span(),
            repeat_token: self.token(TokenKind::Repeat),
            block,
            until_token: self.token(TokenKind::Until),
            condition: cond,
        }))
    }

    pub fn if_(
        &mut self,
        cond: Expression,
        then_block: Block,
        elseifs: Vec<(Expression, Block)>,
        else_block: Option<Block>,
    ) -> Statement {
        let span = self.next_span();
        let if_token = self.token(TokenKind::If);
        let then_token = self.token(TokenKind::Then);
        let elseif_clauses: Vec<ElseIfClause> = elseifs
            .into_iter()
            .map(|(condition, block)| ElseIfClause {
                span: self.next_span(),
                elseif_token: self.token(TokenKind::ElseIf),
                condition,
                then_token: self.token(TokenKind::Then),
                block,
            })
            .collect();
        let else_clause = else_block.map(|block| ElseClause {
            span: self.next_span(),
            else_token: self.token(TokenKind::Else),
            block,
        });
        Statement::IfStatement(Box::new(IfStatement {
            span,
            if_token,
            condition: cond,
            then_token,
            block: then_block,
            elseif_clauses,
            else_clause,
            end_token: self.token(TokenKind::End),
        }))
    }

    pub fn numeric_for(
        &mut self,
        var: &str,
        start: Expression,
        limit: Expression,
        step: Option<Expression>,
        block: Block,
    ) -> Statement {
        Statement::NumericFor(Box::new(NumericFor {
            span: self.next_span(),
            for_token: self.token(TokenKind::For),
            name: self.ident(var),
            type_annotation: None,
            equal: self.token(TokenKind::Equal),
            start,
            comma1: self.token(TokenKind::Comma),
            limit,
            comma2_and_step: step.map(|step| (self.token(TokenKind::Comma), step)),
            do_token: self.token(TokenKind::Do),
            block,
            end_token: self.token(TokenKind::End),
        }))
    }

    pub fn generic_for(
        &mut self,
        names: Vec<&str>,
        exprs: Vec<Expression>,
        block: Block,
    ) -> Statement {
        let params = self.params(names);
        Statement::GenericFor(Box::new(GenericFor {
            span: self.next_span(),
            for_token: self.token(TokenKind::For),
            names: self.punctuated(params),
            in_token: self.token(TokenKind::In),
            exprs: self.punctuated(exprs),
            do_token: self.token(TokenKind::Do),
            block,
            end_token: self.token(TokenKind::End),
        }))
    }

    pub fn function_decl(
        &mut self,
        name_path: Vec<&str>,
        method: Option<&str>,
        params: Vec<&str>,
        body: Block,
    ) -> Statement {
        let names: Vec<Token> = name_path.into_iter().map(|part| self.ident(part)).collect();
        // One `.` sits between each pair of names.
        let dots: Vec<Token> = (1..names.len())
            .map(|_| self.token(TokenKind::Dot))
            .collect();
        let method = method.map(|name| (self.token(TokenKind::Colon), self.ident(name)));
        let name = FuncName {
            span: self.next_span(),
            names,
            dots,
            method,
        };
        let params = self.params(params);
        Statement::FunctionDecl(Box::new(FunctionDecl {
            span: self.next_span(),
            attributes: Vec::new(),
            function_token: self.token(TokenKind::Function),
            name,
            body: self.function_body(params, None, body),
        }))
    }

    /// `function a.b:method(params) body end` - a method declared through
    /// `FuncName.method`. `table_path` is the dotted receiver path.
    pub fn method_decl(
        &mut self,
        table_path: Vec<&str>,
        method: &str,
        params: Vec<&str>,
        body: Block,
    ) -> Statement {
        let names: Vec<Token> = table_path
            .into_iter()
            .map(|part| self.ident(part))
            .collect();
        // One `.` sits between each pair of names.
        let dots: Vec<Token> = (1..names.len())
            .map(|_| self.token(TokenKind::Dot))
            .collect();
        let name = FuncName {
            span: self.next_span(),
            names,
            dots,
            method: Some((self.token(TokenKind::Colon), self.ident(method))),
        };
        let params = self.params(params);
        Statement::FunctionDecl(Box::new(FunctionDecl {
            span: self.next_span(),
            attributes: Vec::new(),
            function_token: self.token(TokenKind::Function),
            name,
            body: self.function_body(params, None, body),
        }))
    }

    pub fn local_function(&mut self, name: &str, params: Vec<&str>, body: Block) -> Statement {
        let params = self.params(params);
        Statement::LocalFunction(Box::new(LocalFunction {
            span: self.next_span(),
            attributes: Vec::new(),
            local_token: self.token(TokenKind::Local),
            function_token: self.token(TokenKind::Function),
            name: self.ident(name),
            body: self.function_body(params, None, body),
        }))
    }

    /// `goto label` (Lua 5.2+).
    pub fn goto_(&mut self, label: &str) -> Statement {
        Statement::Goto(Box::new(GotoStatement {
            span: self.next_span(),
            goto_token: self.token(TokenKind::Goto),
            name: self.ident(label),
        }))
    }

    /// `::name::` label (Lua 5.2+).
    pub fn label(&mut self, name: &str) -> Statement {
        Statement::Label(Box::new(LabelStatement {
            span: self.next_span(),
            colons_open: self.token(TokenKind::DoubleColon),
            name: self.ident(name),
            colons_close: self.token(TokenKind::DoubleColon),
        }))
    }

    pub fn return_(&mut self, exprs: Vec<Expression>) -> LastStatement {
        LastStatement::Return(Box::new(ReturnStatement {
            span: self.next_span(),
            return_token: self.token(TokenKind::Return),
            exprs: self.punctuated(exprs),
            semicolon: None,
        }))
    }

    pub fn break_(&mut self) -> LastStatement {
        let token = self.token(TokenKind::Break);
        LastStatement::Break(token)
    }

    /// Luau `continue`. The keyword is context-sensitive, so it rides in as an
    /// `Identifier`, exactly as the parser produces it.
    pub fn continue_(&mut self) -> LastStatement {
        let token = self.ident("continue");
        LastStatement::Continue(token)
    }

    pub fn block(&mut self, stmts: Vec<Statement>, last: Option<LastStatement>) -> Block {
        Block {
            span: self.next_span(),
            stmts,
            last_stmt: last.map(Box::new),
        }
    }

    pub fn param(&mut self, name: &str) -> Parameter {
        Parameter {
            span: self.next_span(),
            name: self.ident(name),
            type_annotation: None,
        }
    }

    pub fn param_typed(&mut self, name: &str, ty: Type) -> Parameter {
        Parameter {
            span: self.next_span(),
            name: self.ident(name),
            type_annotation: Some((self.token(TokenKind::Colon), ty)),
        }
    }

    /// A trailing `...` parameter, optionally with a Luau pack annotation. The
    /// name stays `None` - Lua 5.5's `...name` form is not synthesized here.
    pub fn vararg_param(&mut self, type_annotation: Option<Type>) -> VarArgParam {
        VarArgParam {
            span: self.next_span(),
            dots: self.token(TokenKind::DotDotDot),
            name: None,
            type_annotation: type_annotation.map(|ty| (self.token(TokenKind::Colon), ty)),
        }
    }

    pub fn ty_named(&mut self, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: None,
            name: self.ident(name),
            generics: None,
        }))
    }

    pub fn ty_qualified(&mut self, module: &str, name: &str) -> Type {
        Type::Named(Box::new(NamedType {
            span: self.next_span(),
            prefix: Some((self.ident(module), self.token(TokenKind::Dot))),
            name: self.ident(name),
            generics: None,
        }))
    }

    pub fn ty_generic(&mut self, name: &str, args: Vec<Type>) -> Type {
        let span = self.next_span();
        let name = self.ident(name);
        let angles = self.contained(TokenKind::Less, TokenKind::Greater);
        let generics = TypeArgs {
            span: self.next_span(),
            angles,
            args: self.punctuated(args),
        };
        Type::Named(Box::new(NamedType {
            span,
            prefix: None,
            name,
            generics: Some(generics),
        }))
    }

    pub fn ty_optional(&mut self, inner: Type) -> Type {
        Type::Optional(Box::new(OptionalType {
            span: self.next_span(),
            type_value: inner,
            question: self.token(TokenKind::Question),
        }))
    }

    pub fn ty_union(&mut self, types: Vec<Type>) -> Type {
        Type::Union(Box::new(UnionType {
            span: self.next_span(),
            leading_pipe: None,
            types: self.punctuated_with(types, TokenKind::Pipe),
        }))
    }

    pub fn ty_intersection(&mut self, types: Vec<Type>) -> Type {
        Type::Intersection(Box::new(IntersectionType {
            span: self.next_span(),
            leading_ampersand: None,
            types: self.punctuated_with(types, TokenKind::Ampersand),
        }))
    }

    pub fn ty_table_array(&mut self, element: Type) -> Type {
        let span = self.next_span();
        let braces = self.contained(TokenKind::LeftBrace, TokenKind::RightBrace);
        let field = TypeField::Array {
            span: self.next_span(),
            value: element,
        };
        Type::Table(Box::new(TableType {
            span,
            braces,
            fields: vec![(field, None)],
        }))
    }

    pub fn ty_table(&mut self, fields: Vec<(String, Type)>) -> Type {
        let span = self.next_span();
        let braces = self.contained(TokenKind::LeftBrace, TokenKind::RightBrace);
        let count = fields.len();
        let mut built = Vec::with_capacity(count);
        for (index, (name, value)) in fields.into_iter().enumerate() {
            let field = TypeField::Named {
                span: self.next_span(),
                access: None,
                name: self.ident(&name),
                colon: self.token(TokenKind::Colon),
                value,
            };
            let separator = if index + 1 < count {
                Some(self.token(TokenKind::Comma))
            } else {
                None
            };
            built.push((field, separator));
        }
        Type::Table(Box::new(TableType {
            span,
            braces,
            fields: built,
        }))
    }

    pub fn ty_function(&mut self, params: Vec<Type>, return_type: Type) -> Type {
        let span = self.next_span();
        let parens = self.contained(TokenKind::LeftParen, TokenKind::RightParen);
        let params: Vec<FunctionTypeParam> = params
            .into_iter()
            .map(|type_value| FunctionTypeParam {
                span: self.next_span(),
                name: None,
                type_value,
            })
            .collect();
        let params = self.punctuated(params);
        Type::Function(Box::new(FunctionType {
            span,
            generics: None,
            parens,
            params,
            arrow: self.token(TokenKind::Arrow),
            return_type,
        }))
    }

    pub fn ty_singleton_string(&mut self, content: &str) -> Type {
        let token = self.token(TokenKind::StringLiteral(raw_string(content)));
        Type::Singleton(token)
    }

    pub fn ty_pack(&mut self, types: Vec<Type>) -> Type {
        let span = self.next_span();
        let parens = self.contained(TokenKind::LeftParen, TokenKind::RightParen);
        Type::Pack(Box::new(TypePack {
            span,
            parens,
            types: self.punctuated(types),
        }))
    }

    pub fn ty_variadic(&mut self, element: Type) -> Type {
        Type::Variadic(Box::new(VariadicType {
            span: self.next_span(),
            dots: self.token(TokenKind::DotDotDot),
            type_value: element,
        }))
    }

    /// A generic parameter list at a declaration site: `<T, U...>`. Each pair is
    /// a name and whether it is a pack (`T...`); defaults are not synthesized.
    pub fn generic_type_list(&mut self, params: Vec<(&str, bool)>) -> GenericTypeList {
        let span = self.next_span();
        let angles = self.contained(TokenKind::Less, TokenKind::Greater);
        let built: Vec<GenericTypeParam> = params
            .into_iter()
            .map(|(name, is_pack)| GenericTypeParam {
                span: self.next_span(),
                name: self.ident(name),
                dots: is_pack.then(|| self.token(TokenKind::DotDotDot)),
                default: None,
            })
            .collect();
        GenericTypeList {
            span,
            angles,
            params: self.punctuated(built),
        }
    }

    /// Luau alias `type Name [<generics>] = T`. The `type` keyword is
    /// context-sensitive, so it rides in as an `Identifier`, as the parser
    /// produces it; `export` and the `type function` form are not synthesized.
    pub fn type_declaration(
        &mut self,
        name: &str,
        generics: Option<GenericTypeList>,
        value: Type,
    ) -> Statement {
        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span: self.next_span(),
            export_token: None,
            type_token: self.ident("type"),
            function_token: None,
            name: self.ident(name),
            generics: generics.map(Box::new),
            equal: Some(self.token(TokenKind::Equal)),
            type_value: TypeDeclarationValue::Alias(value),
        }))
    }

    pub fn comment(
        &mut self,
        anchor_span_start: u32,
        text: &str,
        is_leading: bool,
    ) -> SyntheticComment {
        SyntheticComment {
            attached_to: anchor_span_start,
            text: CompactString::from(text),
            is_leading,
        }
    }

    fn contained(&mut self, open: TokenKind, close: TokenKind) -> ContainedSpan {
        ContainedSpan {
            open: self.token(open),
            close: self.token(close),
        }
    }

    fn paren_args(&mut self, args: Vec<Expression>) -> FunctionArgs {
        FunctionArgs::Parenthesized {
            parens: self.contained(TokenKind::LeftParen, TokenKind::RightParen),
            args: self.punctuated(args),
        }
    }

    fn params(&mut self, names: Vec<&str>) -> Vec<Parameter> {
        names.into_iter().map(|name| self.param(name)).collect()
    }

    fn function_body(
        &mut self,
        params: Vec<Parameter>,
        return_type: Option<Type>,
        block: Block,
    ) -> FunctionBody {
        FunctionBody {
            span: self.next_span(),
            generics: None,
            params_parens: self.contained(TokenKind::LeftParen, TokenKind::RightParen),
            params: self.punctuated(params),
            vararg: None,
            return_type: return_type.map(|ty| (self.token(TokenKind::Colon), ty)),
            block,
            end_token: self.token(TokenKind::End),
        }
    }

    fn punctuated<T>(&mut self, items: Vec<T>) -> Punctuated<T> {
        self.punctuated_with(items, TokenKind::Comma)
    }

    /// Separator token FOLLOWS each item; the last item's is `None`.
    fn punctuated_with<T>(&mut self, items: Vec<T>, separator: TokenKind) -> Punctuated<T> {
        let count = items.len();
        let items = items
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let following = if index + 1 < count {
                    Some(self.token(separator.clone()))
                } else {
                    None
                };
                (item, following)
            })
            .collect();
        Punctuated { items }
    }

    /// Var-shaped expressions only. Any other expression is a synthesis-side
    /// programmer error (a decompiler would never target a non-lvalue), so we
    /// panic rather than thread a `Result` through every call site.
    fn expr_to_var(expr: Expression) -> Var {
        match expr {
            Expression::Var(var) => *var,
            other => panic!(
                "synth: assignment target must be a name, index, or field access, got {other:?}"
            ),
        }
    }
}

/// Double-quoted raw form of `content`: escapes `\`, `"`, newline, carriage
/// return, and tab, and renders other control bytes as Lua `\ddd` decimal
/// escapes. Passthrough bytes keep the original UTF-8 intact because every
/// escaped byte is a single ASCII byte, never part of a multibyte sequence.
fn raw_string(content: &str) -> CompactString {
    let mut raw = Vec::with_capacity(content.len() + 2);
    raw.push(b'"');
    for &byte in content.as_bytes() {
        match byte {
            b'\\' => raw.extend_from_slice(b"\\\\"),
            b'"' => raw.extend_from_slice(b"\\\""),
            b'\n' => raw.extend_from_slice(b"\\n"),
            b'\r' => raw.extend_from_slice(b"\\r"),
            b'\t' => raw.extend_from_slice(b"\\t"),
            0x00..=0x1f | 0x7f => {
                raw.push(b'\\');
                raw.extend_from_slice(format!("{byte}").as_bytes());
            }
            _ => raw.push(byte),
        }
    }
    raw.push(b'"');
    let text = String::from_utf8(raw).expect("synth: string escaping produced invalid UTF-8");
    CompactString::from(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_local_with_binop() {
        let mut synth = Synth::new();
        let one = synth.number("1");
        let two = synth.number("2");
        let sum = synth.binop(one, TokenKind::Plus, two);
        let stmt = synth.local(vec!["x"], vec![sum]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        assert_eq!(local.names.len(), 1);
        let (_, exprs) = local.equal_and_exprs.as_ref().unwrap();
        assert!(matches!(exprs.first().unwrap(), Expression::BinaryOp(_)));
    }

    #[test]
    fn builds_if_with_elseif() {
        let mut synth = Synth::new();
        let cond = synth.bool(true);
        let then_block = synth.block(vec![], None);
        let elseif_cond = synth.bool(false);
        let elseif_block = synth.block(vec![], None);
        let stmt = synth.if_(cond, then_block, vec![(elseif_cond, elseif_block)], None);

        let Statement::IfStatement(if_stmt) = &stmt else {
            panic!("expected if statement");
        };
        assert_eq!(if_stmt.elseif_clauses.len(), 1);
        assert!(if_stmt.else_clause.is_none());
    }

    #[test]
    fn builds_typed_local() {
        let mut synth = Synth::new();
        let number = synth.ty_named("number");
        let optional = synth.ty_optional(number);
        let nil = synth.nil();
        let stmt = synth.local_typed(vec![("x", Some(optional))], vec![nil]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let first = local.names.first().unwrap();
        let (_, ty) = first.type_annotation.as_ref().unwrap();
        assert!(matches!(ty, Type::Optional(_)));
    }

    #[test]
    fn builds_typed_function() {
        let mut synth = Synth::new();
        let param_type = synth.ty_named("number");
        let param = synth.param_typed("n", param_type);
        let return_type = synth.ty_named("number");
        let value = synth.name_expr("n");
        let ret = synth.return_(vec![value]);
        let body = synth.block(vec![], Some(ret));
        let func = synth.function_def_typed(vec![param], Some(return_type), body);

        let Expression::FunctionDef(def) = &func else {
            panic!("expected function def");
        };
        assert_eq!(def.body.params.len(), 1);
        assert!(def.body.params.first().unwrap().type_annotation.is_some());
        assert!(def.body.return_type.is_some());
    }

    #[test]
    fn builds_generic_for() {
        let mut synth = Synth::new();
        let pairs = synth.name_expr("pairs");
        let table = synth.name_expr("t");
        let call = synth.call(pairs, vec![table]);
        let body = synth.block(vec![], None);
        let stmt = synth.generic_for(vec!["k", "v"], vec![call], body);

        let Statement::GenericFor(generic) = &stmt else {
            panic!("expected generic for");
        };
        assert_eq!(generic.names.len(), 2);
        assert_eq!(generic.exprs.len(), 1);
    }

    #[test]
    fn builds_mixed_table() {
        let mut synth = Synth::new();
        let positional = synth.number("1");
        let named_value = synth.number("2");
        let key = synth.string("k");
        let bracket_value = synth.bool(true);
        let table = synth.table(vec![
            SynthField::Positional(positional),
            SynthField::Named("a".to_string(), named_value),
            SynthField::Bracketed(key, bracket_value),
        ]);

        let Expression::TableConstructor(constructor) = &table else {
            panic!("expected table constructor");
        };
        assert_eq!(constructor.fields.len(), 3);
        assert!(matches!(constructor.fields[0].0, Field::Positional { .. }));
        assert!(matches!(constructor.fields[1].0, Field::Named { .. }));
        assert!(matches!(constructor.fields[2].0, Field::Bracketed { .. }));
    }

    #[test]
    fn spans_are_monotonic() {
        let mut synth = Synth::new();
        let first = synth.nil();
        let second = synth.nil();
        let third = synth.number("3");
        assert!(second.span().start > first.span().start);
        assert!(third.span().start > second.span().start);
    }

    #[test]
    fn string_escaping_is_raw() {
        let mut synth = Synth::new();
        let expr = synth.string("a\"b\n");
        let Expression::StringLiteral(token) = &expr else {
            panic!("expected string literal");
        };
        let TokenKind::StringLiteral(raw) = &token.kind else {
            panic!("expected string literal token");
        };
        assert_eq!(raw.as_str(), "\"a\\\"b\\n\"");
    }

    #[test]
    fn builds_variadic_function() {
        let mut synth = Synth::new();
        let param = synth.param("first");
        let body = synth.block(vec![], None);
        let func = synth.function_def_variadic(vec![param], None, None, body);

        let Expression::FunctionDef(def) = &func else {
            panic!("expected function def");
        };
        assert_eq!(def.body.params.len(), 1);
        let vararg = def.body.vararg.as_ref().expect("vararg present");
        assert!(vararg.name.is_none());
        assert!(vararg.type_annotation.is_none());
    }

    #[test]
    fn builds_typed_vararg_param() {
        let mut synth = Synth::new();
        let pack = synth.ty_named("number");
        let vararg = synth.vararg_param(Some(pack));
        assert!(vararg.name.is_none());
        assert!(matches!(vararg.dots.kind, TokenKind::DotDotDot));
        assert!(vararg.type_annotation.is_some());
    }

    #[test]
    fn builds_continue() {
        let mut synth = Synth::new();
        assert!(matches!(synth.continue_(), LastStatement::Continue(_)));
    }

    #[test]
    fn builds_type_cast() {
        let mut synth = Synth::new();
        let value = synth.name_expr("x");
        let target_type = synth.ty_named("number");
        let cast = synth.type_cast(value, target_type);

        let Expression::TypeCast(node) = &cast else {
            panic!("expected type cast");
        };
        assert!(matches!(node.double_colon.kind, TokenKind::DoubleColon));
        assert!(matches!(node.expr, Expression::Var(_)));
    }

    #[test]
    fn builds_compound_assignment() {
        let mut synth = Synth::new();
        let target = synth.name_expr("counter");
        let value = synth.number("1");
        let stmt = synth.compound_assign(target, TokenKind::PlusEqual, value);

        let Statement::CompoundAssignment(compound) = &stmt else {
            panic!("expected compound assignment");
        };
        assert!(matches!(compound.var, Var::Name(_)));
        assert!(matches!(compound.op.kind, TokenKind::PlusEqual));
    }

    #[test]
    #[should_panic(expected = "compound-assignment operator")]
    fn compound_assignment_rejects_plain_operator() {
        let mut synth = Synth::new();
        let target = synth.name_expr("counter");
        let value = synth.number("1");
        synth.compound_assign(target, TokenKind::Plus, value);
    }

    #[test]
    fn builds_interpolated_string() {
        let mut synth = Synth::new();
        let inner = synth.name_expr("value");
        // `a{value}b` -> InterpBegin("a") + expr, then InterpEnd("b") terminator.
        let string = synth.interpolated_string(vec![
            ("a".to_string(), Some(inner)),
            ("b".to_string(), None),
        ]);

        let Expression::InterpolatedString(interp) = &string else {
            panic!("expected interpolated string");
        };
        assert_eq!(interp.segments.len(), 2);
        assert!(matches!(
            interp.segments[0].literal.kind,
            TokenKind::InterpBegin(_)
        ));
        assert!(interp.segments[0].expr.is_some());
        assert!(matches!(
            interp.segments[1].literal.kind,
            TokenKind::InterpEnd(_)
        ));
        assert!(interp.segments[1].expr.is_none());
    }

    #[test]
    fn builds_goto_and_label() {
        let mut synth = Synth::new();
        let goto = synth.goto_("done");
        let label = synth.label("done");
        assert!(matches!(goto, Statement::Goto(_)));
        assert!(matches!(label, Statement::Label(_)));
    }

    #[test]
    fn builds_type_declaration_with_generics() {
        let mut synth = Synth::new();
        let generics = synth.generic_type_list(vec![("T", false), ("Rest", true)]);
        let value = synth.ty_named("T");
        let stmt = synth.type_declaration("Alias", Some(generics), value);

        let Statement::TypeDeclaration(decl) = &stmt else {
            panic!("expected type declaration");
        };
        assert!(decl.export_token.is_none());
        assert!(decl.equal.is_some());
        let generics = decl.generics.as_ref().expect("generics present");
        assert_eq!(generics.params.len(), 2);
        // The pack parameter carries `...`.
        assert!(generics.params.last_item().unwrap().dots.is_some());
    }

    #[test]
    fn builds_attributed_local() {
        let mut synth = Synth::new();
        let value = synth.number("1");
        let stmt = synth.attributed_local(vec![("frozen", Some("const"))], vec![value]);

        let Statement::LocalAssignment(local) = &stmt else {
            panic!("expected local assignment");
        };
        let first = local.names.first().unwrap();
        let attribute = first.attrib.as_ref().expect("attribute present");
        assert!(matches!(attribute.open.kind, TokenKind::Less));
        assert!(matches!(attribute.close.kind, TokenKind::Greater));
    }

    #[test]
    fn builds_method_declaration() {
        let mut synth = Synth::new();
        let body = synth.block(vec![], None);
        let stmt = synth.method_decl(vec!["a", "b"], "greet", vec!["self"], body);

        let Statement::FunctionDecl(decl) = &stmt else {
            panic!("expected function declaration");
        };
        assert_eq!(decl.name.names.len(), 2);
        assert_eq!(decl.name.dots.len(), 1);
        assert!(decl.name.method.is_some());
    }
}
