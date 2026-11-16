use luck_ast::{
    BinaryLink, BinaryOp, CallSuffix, ElseIfExprClause, Expression, Field, FunctionArgs,
    FunctionDef, IfExpression, InterpolatedString, InterpolatedStringSegment, MethodCall,
    OneOrMore, ParenthesizedExpression, Punctuated, Suffix, SuffixedExpression, TableConstructor,
    TypeArgs, TypeCast, UnaryOp,
};
use luck_token::{BinOp, Span, Token, TokenKind, UnOp};

use super::{Closer, Parser};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// An expression a level down, as `subexpr` and `parseExpr` count it.
    #[inline]
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression {
        let span = self.current_span();
        if !self.enter_depth() {
            return Expression::Error(span);
        }
        let expr = self.parse_expr_inner(min_precedence);
        self.exit_depth();
        expr
    }

    /// An expression on the parser's own level, as a statement reads the
    /// call or target it opens with. Each precedence level's operators run in
    /// a loop into one node, whose operands after the first are each a level.
    pub(super) fn parse_expr_inner(&mut self, min_precedence: u8) -> Expression {
        let lhs = self.parse_prefix();
        if self.peek_binary_op().is_some_and(|op| op.precedence().0 >= min_precedence) {
            return self.parse_binary_runs(lhs, min_precedence);
        }
        lhs
    }

    /// Apart from [`Self::parse_expr_inner`], so an operand with no operator
    /// after it keeps the loop off the stack under it.
    #[inline(never)]
    fn parse_binary_runs(&mut self, mut lhs: Expression, min_precedence: u8) -> Expression {
        while let Some(op) = self.peek_binary_op() {
            let precedence = op.precedence().0;
            if precedence < min_precedence {
                break;
            }
            self.advance_span();
            // The run's operands bind tighter, so a right-associative run
            // loops like a left-associative one and only the grouping differs.
            let operand_precedence = precedence.saturating_add(1);
            let first_link = BinaryLink { op, operand: self.parse_expr(operand_precedence) };
            let start = self.binary_links.len();
            while let Some(next_op) =
                self.peek_binary_op().filter(|next_op| next_op.precedence().0 == precedence)
            {
                self.advance_span();
                let operand = self.parse_expr(operand_precedence);
                self.binary_links.push(BinaryLink { op: next_op, operand });
            }
            let links = OneOrMore::from_first(first_link, self.binary_links.drain(start..));
            let span = lhs.span().merge(links.last().operand.span());
            lhs = Expression::BinaryOp(Box::new(BinaryOp { span, first: lhs, links }));
        }
        lhs
    }

    fn peek_binary_op(&self) -> Option<BinOp> {
        let op = BinOp::from_token_kind(self.peek())?;
        // Luau's lexer emits `&` and `|` for the type grammar only; they
        // aren't binary operators there.
        (!op.is_bitwise() || self.version.has_bitwise_ops()).then_some(op)
    }

    pub(super) fn parse_expr_list(&mut self) -> Punctuated<Expression> {
        let first = self.parse_expr(0);
        Punctuated::from_items(self.parse_exprs_after(first, Self::parse_expr))
    }

    /// `first` and the expression `parse` reads after each comma that
    /// follows it.
    pub(super) fn parse_exprs_after<Parse>(
        &mut self,
        first: Expression,
        parse: Parse,
    ) -> Vec<Expression>
    where
        Parse: Fn(&mut Self, u8) -> Expression,
    {
        if !matches!(self.peek(), TokenKind::Comma) {
            return vec![first];
        }
        let start = self.exprs.len();
        self.exprs.push(first);
        while self.eat(&TokenKind::Comma) {
            let expr = parse(self, 0);
            self.exprs.push(expr);
        }
        self.exprs.drain(start..).collect()
    }

    fn parse_prefix(&mut self) -> Expression {
        if let Some(op) = UnOp::from_token_kind(self.peek()) {
            return self.parse_unary_op(op);
        }
        let primary = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        if self.version.has_type_syntax() && matches!(self.peek(), TokenKind::DoubleColon) {
            return self.parse_type_cast(primary);
        }
        primary
    }

    #[inline(never)]
    fn parse_unary_op(&mut self, op: UnOp) -> Expression {
        let op_span = self.advance_span();
        let operand = self.parse_expr(UnOp::PRECEDENCE);
        let span = op_span.merge(operand.span());
        Expression::UnaryOp(Box::new(UnaryOp { span, op, operand }))
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    #[inline(never)]
    fn parse_type_cast(&mut self, expr: Expression) -> Expression {
        self.advance_span();
        let type_annotation = self.parse_type();
        let span = expr.span().merge(type_annotation.span());
        Expression::TypeCast(Box::new(TypeCast { span, expr, type_annotation }))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_primary_expr(&mut self) -> Expression {
        match self.peek() {
            TokenKind::Nil => Expression::Nil(self.advance_span()),
            TokenKind::False => Expression::False(self.advance_span()),
            TokenKind::True => Expression::True(self.advance_span()),
            TokenKind::Number(_) => Expression::Number(self.advance_literal()),
            // Luau
            TokenKind::Integer(_) => Expression::Integer(self.advance_literal()),
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
                self.open(Closer::Paren);
                let expr = self.parse_expr(0);
                let close = self.expect_closer(&TokenKind::RightParen, Closer::Paren);
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
                let name = Expression::Name(self.advance());
                self.parse_suffixes(name)
            }
            _ => self.report_unexpected_in_expr(),
        }
    }

    #[cold]
    #[inline(never)]
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn report_unexpected_in_expr(&mut self) -> Expression {
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
        let message = format!("unexpected token {kind} in an expression{hint}");
        self.syntax_error(span, message);
        Expression::Error(span)
    }

    /// The suffixes after `prefix` loop into one node, which shares the
    /// prefix's level however long it is.
    fn parse_suffixes(&mut self, prefix: Expression) -> Expression {
        if !self.is_at_suffix() {
            return prefix;
        }
        // Every suffix waits in `self.suffixes`, so none is held on the stack
        // under the ones after it.
        let start = self.suffixes.len();
        while self.is_at_suffix() {
            let suffix = self.parse_suffix();
            self.suffixes.push(suffix);
        }
        self.finish_suffixes(prefix, start)
    }

    /// Builds the node apart from [`Self::parse_suffixes`], whose frame sits
    /// under everything nested in the suffixes.
    #[inline(never)]
    fn finish_suffixes(&mut self, prefix: Expression, start: usize) -> Expression {
        let mut suffixes = self.suffixes.drain(start..);
        let Some(first) = suffixes.next() else {
            return prefix;
        };
        let suffixes = OneOrMore::from_first(first, suffixes);
        let span = prefix.span().merge(suffixes.last().span());
        Expression::Suffixed(Box::new(SuffixedExpression { span, prefix, suffixes }))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token ends the run")]
    const fn is_at_suffix(&self) -> bool {
        match self.peek() {
            TokenKind::Less => self.is_at_type_instantiation(),
            TokenKind::Dot
            | TokenKind::LeftBracket
            | TokenKind::Colon
            | TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral(_) => true,
            _ => false,
        }
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other suffix is a call's arguments")]
    fn parse_suffix(&mut self) -> Suffix {
        match self.peek() {
            // Luau: explicit type-parameter instantiation is a suffix, so it
            // composes with field access, indexing, and calls.
            TokenKind::Less => {
                let open = self.current_span();
                let (type_args, close) = self.parse_type_instantiation();
                Suffix::TypeInstantiation {
                    span: open.merge(close),
                    type_args: Box::new(type_args),
                }
            }
            TokenKind::Dot => {
                self.advance_span();
                Suffix::Field(self.expect_identifier_recover())
            }
            TokenKind::LeftBracket => {
                let open = self.advance_span();
                self.open(Closer::Bracket);
                let index = self.parse_expr(0);
                let close = self.expect_closer(&TokenKind::RightBracket, Closer::Bracket);
                Suffix::Index { span: open.merge(close), index }
            }
            TokenKind::Colon => self.parse_method_suffix(),
            _ => Suffix::Call(CallSuffix::Plain(self.parse_function_args())),
        }
    }

    #[inline(never)]
    fn parse_method_suffix(&mut self) -> Suffix {
        self.advance_span();
        let name = self.expect_identifier_recover();
        let explicit_type_args =
            self.is_at_type_instantiation().then(|| Box::new(self.parse_type_instantiation().0));
        let args = self.parse_function_args();
        let call = MethodCall { name, explicit_type_args, args };
        Suffix::Call(CallSuffix::Method(Box::new(call)))
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
        self.open(Closer::Else);
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

        self.close(Closer::Else);
        self.expect(&TokenKind::Else);
        let else_expr = self.parse_expr(0);
        let span = if_span.merge(else_expr.span());

        Expression::IfExpression(Box::new(IfExpression {
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
        }))
    }

    fn parse_interpolated_string(&mut self) -> Expression {
        let start_span = self.current_span();
        let begin_token = self.advance();
        let mut segments = Vec::new();
        self.open(Closer::StringSegment);
        // A plain string lexes as an empty `InterpolatedStringBegin` and an
        // `InterpolatedStringEnd` that lies inside it, where a `{}` puts the
        // end past it.
        if matches!(self.peek(), TokenKind::InterpolatedStringEnd(_))
            && self.current.span.end <= begin_token.span.end
        {
            segments.push(InterpolatedStringSegment { literal: begin_token, expr: None });
        } else {
            self.parse_interpolated_hole(begin_token, &mut segments);
            while matches!(self.peek(), TokenKind::InterpolatedStringMiddle(_)) {
                let middle_token = self.advance();
                self.parse_interpolated_hole(middle_token, &mut segments);
            }
        }
        let end_span = if matches!(self.peek(), TokenKind::InterpolatedStringEnd(_)) {
            let end_token = self.advance();
            let span = end_token.span;
            segments.push(InterpolatedStringSegment { literal: end_token, expr: None });
            span
        } else {
            self.report_unfinished_interpolated_string()
        };
        self.close(Closer::StringSegment);
        let span = start_span.merge(end_span);
        Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
    }

    /// The expression after `literal`, whose last byte is the `{` that opens
    /// it. A `{}` with none has its `}` at the current token.
    fn parse_interpolated_hole(
        &mut self,
        literal: Token,
        segments: &mut Vec<InterpolatedStringSegment>,
    ) {
        let expr = if matches!(
            self.peek(),
            TokenKind::InterpolatedStringMiddle(_) | TokenKind::InterpolatedStringEnd(_)
        ) {
            let span = Span::new(
                literal.span.end.saturating_sub(1),
                self.current.span.start.saturating_add(1),
            );
            self.error(span, "malformed interpolated string: expected expression inside `{}`");
            Expression::Error(span)
        } else {
            self.parse_expr(0)
        };
        segments.push(InterpolatedStringSegment { literal, expr: Some(expr) });
    }

    #[cold]
    #[inline(never)]
    fn report_unfinished_interpolated_string(&mut self) -> Span {
        let span = self.current_span();
        let message = format!("expected interpolated string continuation, found {}", self.peek());
        self.syntax_error(span, message);
        span
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
                self.open(Closer::Paren);
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expr_list()
                };
                let close = self.expect_closer(&TokenKind::RightParen, Closer::Paren);
                FunctionArgs::Parenthesized { span: open.merge(close), args }
            }
            TokenKind::LeftBrace => {
                FunctionArgs::TableConstructor(Box::new(self.parse_table_constructor()))
            }
            TokenKind::StringLiteral(_) => FunctionArgs::StringLiteral(self.advance_literal()),
            _ => {
                let span = self.current_span();
                self.syntax_error(span, "expected function arguments");
                FunctionArgs::Error(span)
            }
        }
    }

    fn parse_function_def(&mut self) -> Expression {
        let attributes = self.parse_function_attributes();
        let start_span =
            attributes.first().map_or_else(|| self.current_span(), |attribute| attribute.span);
        if !matches!(self.peek(), TokenKind::Function) {
            let span = self.current_span();
            self.syntax_error(span, "expected `function` after attribute");
            return Expression::Error(span);
        }
        self.advance_span(); // `function`
        let body = self.parse_function_body();
        let span = start_span.merge(body.span);
        Expression::FunctionDef(Box::new(FunctionDef { span, attributes, body }))
    }

    pub(super) fn parse_table_constructor(&mut self) -> TableConstructor {
        let start_span = self.advance_span(); // `{`
        self.open(Closer::Brace);

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

        let close = self.expect_closer(&TokenKind::RightBrace, Closer::Brace);
        let span = start_span.merge(close);

        TableConstructor { span, fields }
    }

    fn parse_field(&mut self) -> Field {
        if matches!(self.peek(), TokenKind::LeftBracket) {
            let open = self.advance_span();
            self.open(Closer::Bracket);
            let key = self.parse_expr(0);
            self.expect_closer(&TokenKind::RightBracket, Closer::Bracket);
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
