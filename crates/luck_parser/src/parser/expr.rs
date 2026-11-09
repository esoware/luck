use luck_ast::{
    BinaryOp, ElseIfExprClause, Expression, Field, FieldAccess, FunctionArgs, FunctionCall,
    FunctionDef, IfExpression, IndexExpression, InterpolatedString, InterpolatedStringSegment,
    ParenthesizedExpression, Punctuated, TableConstructor, TypeArgs, TypeCast, TypeInstantiation,
    UnaryOp, Var,
};
use luck_token::{Associativity, BinOp, Span, TokenKind, UnOp};

use super::Parser;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// Left-associative chains grow in a loop; only right operands recurse.
    #[inline]
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression {
        let mut lhs = self.parse_prefix();

        while let Some(op) = BinOp::from_token_kind(self.peek()) {
            // Luau's lexer emits `&` and `|` for the type grammar only; they
            // aren't binary operators there.
            if op.is_bitwise() && !self.version.has_bitwise_ops() {
                break;
            }
            let (precedence, associativity) = op.precedence();
            if precedence < min_precedence {
                break;
            }
            let op_span = self.advance_span();
            let rhs_min_precedence = if associativity == Associativity::Right {
                precedence
            } else {
                precedence.saturating_add(1)
            };
            // Right-associative chains (`..`, `^`) recurse once per operator,
            // so a long one would overflow the stack without this guard.
            if !self.enter_depth() {
                let span = lhs.span().merge(op_span);
                return Expression::BinaryOp(Box::new(BinaryOp {
                    span,
                    lhs,
                    op,
                    rhs: Expression::Error(op_span),
                }));
            }
            let rhs = self.parse_expr(rhs_min_precedence);
            self.exit_depth();
            let span = lhs.span().merge(rhs.span());
            lhs = Expression::BinaryOp(Box::new(BinaryOp { span, lhs, op, rhs }));
        }

        lhs
    }

    pub(super) fn parse_expr_list(&mut self) -> Punctuated<Expression> {
        let mut exprs = vec![self.parse_expr(0)];

        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr(0));
        }

        Punctuated::from_items(exprs)
    }

    fn parse_prefix(&mut self) -> Expression {
        if let Some(op) = UnOp::from_token_kind(self.peek()) {
            let op_span = self.advance_span();
            // Unary chains like `not not not x` recurse once per operator, so
            // they count toward the depth limit too.
            if !self.enter_depth() {
                return Expression::Error(op_span);
            }
            let operand = self.parse_expr(UnOp::PRECEDENCE);
            self.exit_depth();
            let span = op_span.merge(operand.span());
            return Expression::UnaryOp(Box::new(UnaryOp { span, op, operand }));
        }
        let primary = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        self.parse_type_cast(primary)
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    fn parse_type_cast(&mut self, expr: Expression) -> Expression {
        if self.version.has_type_syntax() && self.eat(&TokenKind::DoubleColon) {
            let type_annotation = self.parse_type();
            let span = expr.span().merge(type_annotation.span());
            return Expression::TypeCast(Box::new(TypeCast { span, expr, type_annotation }));
        }
        expr
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_primary_expr(&mut self) -> Expression {
        match self.peek() {
            TokenKind::Nil => Expression::Nil(self.advance_span()),
            TokenKind::False => Expression::False(self.advance_span()),
            TokenKind::True => Expression::True(self.advance_span()),
            TokenKind::Number(_) => self.parse_number(),
            TokenKind::StringLiteral(_) => Expression::StringLiteral(self.advance_literal()),
            TokenKind::DotDotDot => {
                let span = self.advance_span();
                if !self.function_scope.is_vararg {
                    self.error(span, "cannot use `...` outside a vararg function");
                }
                Expression::VarArg(span)
            }
            TokenKind::Function => self.parse_function_def(),
            // Luau: `simpleexp ::= attributes 'function' funcbody`
            TokenKind::At if self.version.has_function_attributes() => self.parse_function_def(),
            TokenKind::LeftBrace => {
                Expression::TableConstructor(Box::new(self.parse_table_constructor()))
            }
            TokenKind::LeftParen => {
                let open = self.advance_span();
                if !self.enter_depth() {
                    return Expression::Error(open);
                }
                let expr = self.parse_expr(0);
                self.exit_depth();
                let close = self.expect(&TokenKind::RightParen);
                let span = open.merge(close);
                let paren_expr =
                    Expression::Parenthesized(Box::new(ParenthesizedExpression { span, expr }));
                self.parse_suffixes(paren_expr)
            }
            // Luau if-expression.
            TokenKind::If if self.version.has_if_exprs() => self.parse_if_expr(),
            // Luau interpolated string.
            TokenKind::InterpolatedStringBegin(_) => self.parse_interpolated_string(),
            TokenKind::Identifier(_) => {
                let name_token = self.advance();
                let var_expr = Expression::Var(Var::Name(name_token));
                self.parse_suffixes(var_expr)
            }
            _ => {
                let span = self.current_span();
                let kind = self.peek();
                let hint = match kind {
                    TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until => {
                        " (possible missing expression before keyword)"
                    }
                    TokenKind::RightParen => " (unmatched `)`)",
                    TokenKind::RightBrace => " (unmatched `}`)",
                    TokenKind::RightBracket => " (unmatched `]`)",
                    TokenKind::Equal => " (use `==` for comparison, not `=`)",
                    _ => "",
                };
                self.error(span, format!("unexpected token {kind} in an expression{hint}"));
                Expression::Error(span)
            }
        }
    }

    // Its own frame keeps these temporaries off that of
    // `parse_primary_expr`, which every level of nesting pays for in an
    // unoptimized build.
    fn parse_number(&mut self) -> Expression {
        let literal = self.advance_literal();
        if literal.text.ends_with('i') {
            Expression::Integer(literal)
        } else {
            Expression::Number(literal)
        }
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_suffixes(&mut self, mut expr: Expression) -> Expression {
        loop {
            match self.peek() {
                // Luau: explicit type-parameter instantiation is a suffix,
                // so it composes with field access, indexing, and calls.
                TokenKind::Less if self.is_at_type_instantiation() => {
                    let (type_args, close) = self.parse_type_instantiation();
                    let span = expr.span().merge(close);
                    expr = Expression::TypeInstantiation(Box::new(TypeInstantiation {
                        span,
                        expr,
                        type_args,
                    }));
                }
                TokenKind::Dot => {
                    self.advance_span();
                    let name = self.expect_identifier_recover();
                    let span = expr.span().merge(name.span);
                    expr = Expression::Var(Var::FieldAccess(Box::new(FieldAccess {
                        span,
                        prefix: expr,
                        name,
                    })));
                }
                TokenKind::LeftBracket => {
                    let open = self.advance_span();
                    if !self.enter_depth() {
                        return Expression::Error(open);
                    }
                    let index = self.parse_expr(0);
                    self.exit_depth();
                    let close = self.expect(&TokenKind::RightBracket);
                    let span = expr.span().merge(close);
                    expr = Expression::Var(Var::Index(Box::new(IndexExpression {
                        span,
                        prefix: expr,
                        index,
                    })));
                }
                TokenKind::Colon => {
                    self.advance_span();
                    let method_name = self.expect_identifier_recover();
                    let explicit_type_args = self
                        .is_at_type_instantiation()
                        .then(|| Box::new(self.parse_type_instantiation().0));
                    let args = self.parse_function_args();
                    let span = expr.span().merge(args.span());
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: Some(method_name),
                        explicit_type_args,
                    }));
                }
                TokenKind::LeftParen | TokenKind::LeftBrace | TokenKind::StringLiteral(_) => {
                    let args = self.parse_function_args();
                    let span = expr.span().merge(args.span());
                    expr = Expression::FunctionCall(Box::new(FunctionCall {
                        span,
                        callee: expr,
                        args,
                        method: None,
                        explicit_type_args: None,
                    }));
                }
                _ => break,
            }
        }
        expr
    }

    const fn is_at_type_instantiation(&self) -> bool {
        self.version.has_explicit_type_instantiation()
            && matches!(self.peek(), TokenKind::Less)
            && matches!(self.peek_next(), TokenKind::Less)
    }

    /// Parse Luau's `<<T, U>>`, returning the type arguments and the span of
    /// the final `>`.
    fn parse_type_instantiation(&mut self) -> (TypeArgs, Span) {
        self.advance_span(); // first `<`
        let type_args = self.parse_type_args(); // second `<` through first `>`
        let close = self.expect(&TokenKind::Greater);
        (type_args, close)
    }

    fn parse_if_expr(&mut self) -> Expression {
        let if_span = self.advance_span();
        if !self.enter_depth() {
            return Expression::Error(if_span);
        }
        let condition = self.parse_expr(0);
        self.expect(&TokenKind::Then);
        let then_expr = self.parse_expr(0);

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_span = self.advance_span();
            let elseif_condition = self.parse_expr(0);
            self.expect(&TokenKind::Then);
            let elseif_expr = self.parse_expr(0);
            let span = elseif_span.merge(elseif_expr.span());
            elseif_clauses.push(ElseIfExprClause {
                span,
                condition: elseif_condition,
                expr: elseif_expr,
            });
        }

        self.expect(&TokenKind::Else);
        let else_expr = self.parse_expr(0);
        self.exit_depth();
        let span = if_span.merge(else_expr.span());

        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
        }))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_interpolated_string(&mut self) -> Expression {
        let begin_token = self.advance();
        let start_span = begin_token.span;
        let mut segments = Vec::new();

        if let TokenKind::InterpolatedStringEnd(_) = self.peek() {
            let end_token = self.advance();
            // A plain string lexes as `InterpolatedStringBegin("")` plus an `InterpolatedStringEnd`
            // that points back inside the begin token. A detached `InterpolatedStringEnd`
            // means `{}` held no expression, which Luau rejects.
            if end_token.span.start >= begin_token.span.end {
                self.error(
                    end_token.span,
                    "malformed interpolated string: expected expression inside `{}`",
                );
            }
            segments.push(InterpolatedStringSegment { literal: begin_token, expr: None });
            let span = start_span.merge(end_token.span);
            segments.push(InterpolatedStringSegment { literal: end_token, expr: None });
            return Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }));
        }

        segments.push(InterpolatedStringSegment {
            literal: begin_token,
            expr: Some(self.parse_interpolated_hole()),
        });

        loop {
            match self.peek() {
                TokenKind::InterpolatedStringMiddle(_) => {
                    let middle_token = self.advance();
                    segments.push(InterpolatedStringSegment {
                        literal: middle_token,
                        expr: Some(self.parse_interpolated_hole()),
                    });
                }
                TokenKind::InterpolatedStringEnd(_) => {
                    let end_token = self.advance();
                    let span = start_span.merge(end_token.span);
                    segments.push(InterpolatedStringSegment { literal: end_token, expr: None });
                    return Expression::InterpolatedString(Box::new(InterpolatedString {
                        span,
                        segments,
                    }));
                }
                _ => {
                    let span = self.current_span();
                    self.error(
                        span,
                        format!("expected interpolated string continuation, found {}", self.peek()),
                    );
                    return Expression::InterpolatedString(Box::new(InterpolatedString {
                        span: start_span.merge(span),
                        segments,
                    }));
                }
            }
        }
    }

    fn parse_interpolated_hole(&mut self) -> Expression {
        if !self.enter_depth() {
            return Expression::Error(self.current_span());
        }
        let expr = self.parse_expr(0);
        self.exit_depth();
        expr
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_function_args(&mut self) -> FunctionArgs {
        match self.peek() {
            TokenKind::LeftParen => {
                // 5.1 and Luau reject a call whose `(` starts on a new line
                // ("ambiguous syntax: function call x new statement"); 5.2+
                // dropped the restriction.
                if self.version.has_ambiguous_call_newline_error()
                    && self.has_newline_before_current()
                {
                    let span = self.current_span();
                    self.error(
                        span,
                        "ambiguous syntax (function call x new statement): put `(` on the same line",
                    );
                }
                let open = self.advance_span();
                if !self.enter_depth() {
                    return FunctionArgs::Parenthesized { span: open, args: Punctuated::empty() };
                }
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expr_list()
                };
                self.exit_depth();
                let close = self.expect(&TokenKind::RightParen);
                FunctionArgs::Parenthesized { span: open.merge(close), args }
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                FunctionArgs::TableConstructor(Box::new(table))
            }
            TokenKind::StringLiteral(_) => FunctionArgs::StringLiteral(self.advance_literal()),
            _ => {
                let span = self.current_span();
                self.error(span, "expected function arguments");
                FunctionArgs::Parenthesized { span, args: Punctuated::empty() }
            }
        }
    }

    fn parse_function_def(&mut self) -> Expression {
        let attributes = self.parse_function_attributes();
        let start_span =
            attributes.first().map_or_else(|| self.current_span(), |attribute| attribute.span);
        if !matches!(self.peek(), TokenKind::Function) {
            let span = self.current_span();
            self.error(span, "expected `function` after attribute");
            return Expression::Error(span);
        }
        self.advance_span(); // `function`
        let body = self.parse_function_body();
        let span = start_span.merge(body.span);
        Expression::FunctionDef(Box::new(FunctionDef { span, attributes, body }))
    }

    pub(super) fn parse_table_constructor(&mut self) -> TableConstructor {
        let start_span = self.advance_span(); // `{`

        if !self.enter_depth() {
            return TableConstructor { span: start_span, fields: Punctuated::empty() };
        }

        let mut fields = Punctuated::empty();

        // Every Lua version rejects a leading separator (`{;}`, `{, 1}`), and
        // so does Luau. Consume it so parsing recovers.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon) {
            let span = self.current_span();
            self.error(span, "unexpected separator in table constructor");
            self.advance_span();
        }

        loop {
            if matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
                break;
            }

            fields.push(self.parse_field());
            let has_separator = matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon);
            if has_separator {
                self.advance_span();
            }
            fields.has_trailing_separator = has_separator;

            if !has_separator {
                break;
            }
        }

        self.exit_depth();

        let close = self.expect(&TokenKind::RightBrace);
        let span = start_span.merge(close);

        TableConstructor { span, fields }
    }

    fn parse_field(&mut self) -> Field {
        // `[expr] = expr`
        if matches!(self.peek(), TokenKind::LeftBracket) {
            let open = self.advance_span();
            let key = self.parse_expr(0);
            self.expect(&TokenKind::RightBracket);
            self.expect(&TokenKind::Equal);
            let value = self.parse_expr(0);
            let span = open.merge(value.span());
            return Field::Bracketed { span, key, value };
        }

        // `Name = expr`, which needs one token of lookahead for the `=`.
        if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Equal) {
            let name = self.advance();
            self.advance_span();
            let value = self.parse_expr(0);
            let span = name.span.merge(value.span());
            return Field::Named { span, name, value };
        }

        let value = self.parse_expr(0);
        let span = value.span();
        Field::Positional { span, value }
    }
}
