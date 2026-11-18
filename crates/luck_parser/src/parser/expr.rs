use luck_allocator::{Box, List};
use luck_ast::{
    BinaryLink, ElseIfExprClause, Expression, Field, FunctionArgs, GetSpan as _,
    InterpolatedSegment, Literal, OneOrMore, Suffix, TableConstructor, TypeArgs,
};
use luck_token::{BinOp, Span, Token, TokenKind, UnOp};

use super::{Closer, Parser};

impl<'ast> Parser<'ast> {
    /// Each precedence level's operators run in a loop into one node, so a
    /// run of any length is one level of nesting, counted toward the depth
    /// cap from its first operand's deepest level.
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression<'ast> {
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let mut lhs = self.parse_prefix();
        while let Some(op) = self.peek_binary_op() {
            let precedence = op.precedence().0;
            if precedence < min_precedence {
                break;
            }
            if !self.enter_wrapper(level) {
                lhs = Expression::Error(lhs.span());
                break;
            }
            lhs = self.parse_binary_run(lhs, op, precedence);
            self.nesting_depth = level;
        }
        self.end_operand(outer);
        lhs
    }

    /// The run's operands bind tighter, so a right-associative run loops like
    /// a left-associative one and only the grouping differs.
    fn parse_binary_run(
        &mut self,
        first: Expression<'ast>,
        op: BinOp,
        precedence: u8,
    ) -> Expression<'ast> {
        self.advance();
        let operand_precedence = precedence.saturating_add(1);
        let start = self.scratch.binary_links.len();
        let operand = self.parse_expr(operand_precedence);
        self.scratch.binary_links.push(BinaryLink::new(op, operand));
        while let Some(next_op) =
            self.peek_binary_op().filter(|next_op| next_op.precedence().0 == precedence)
        {
            self.advance();
            let next_operand = self.parse_expr(operand_precedence);
            self.scratch.binary_links.push(BinaryLink::new(next_op, next_operand));
        }
        let links = OneOrMore::split_off_in(&mut self.scratch.binary_links, start, self.allocator);
        let span = first.span().merge(links.last().operand.span());
        Expression::new_binary_op(span, first, links, self.allocator)
    }

    #[inline]
    fn peek_binary_op(&self) -> Option<BinOp> {
        let op = BinOp::from_token_kind(self.peek())?;
        // Luau's lexer emits `&` and `|` for the type grammar only; they
        // aren't binary operators there.
        (!op.is_bitwise() || self.version.has_bitwise_ops()).then_some(op)
    }

    pub(super) fn parse_expr_list(&mut self) -> List<'ast, Expression<'ast>> {
        let first = self.parse_expr(0);
        if !self.is_at(TokenKind::Comma) {
            return List::from_array_in([first], self.allocator);
        }
        let start = self.scratch.exprs.len();
        self.scratch.exprs.push(first);
        while self.eat(TokenKind::Comma) {
            let expr = self.parse_expr(0);
            self.scratch.exprs.push(expr);
        }
        List::split_off_in(&mut self.scratch.exprs, start, self.allocator)
    }

    #[inline]
    fn parse_prefix(&mut self) -> Expression<'ast> {
        if let Some(op) = UnOp::from_token_kind(self.peek()) {
            return self.parse_unary(op);
        }
        let level = self.nesting_depth;
        let primary = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        if self.is_at(TokenKind::DoubleColon) && self.version.has_type_syntax() {
            return self.parse_type_cast(primary, level);
        }
        primary
    }

    /// Unary chains like `not not not x` recurse once per operator, so they
    /// count toward the depth limit too.
    fn parse_unary(&mut self, op: UnOp) -> Expression<'ast> {
        let op_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(op_span);
        }
        self.advance();
        let operand = self.parse_expr(UnOp::PRECEDENCE);
        self.exit_depth();
        let span = op_span.merge(operand.span());
        Expression::new_unary_op(span, op, operand, self.allocator)
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    fn parse_type_cast(&mut self, expr: Expression<'ast>, level: usize) -> Expression<'ast> {
        if !self.enter_wrapper(level) {
            return Expression::Error(expr.span());
        }
        self.advance();
        let ty = self.parse_type();
        self.nesting_depth = level;
        let span = expr.span().merge(ty.span());
        Expression::new_type_cast(span, expr, ty, self.allocator)
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    #[inline]
    fn parse_primary_expr(&mut self) -> Expression<'ast> {
        match self.peek() {
            TokenKind::Identifier => {
                let name = self.advance_name();
                self.counts.name_uses = self.counts.name_uses.wrapping_add(1);
                let prefix = Expression::Name(self.alloc(name));
                self.parse_suffixes(prefix)
            }
            TokenKind::Number => Expression::Number(self.advance_literal()),
            TokenKind::StringLiteral => Expression::StringLiteral(self.advance_literal()),
            TokenKind::Nil => Expression::Nil(self.advance()),
            TokenKind::False => Expression::False(self.advance()),
            TokenKind::True => Expression::True(self.advance()),
            TokenKind::LeftParen => self.parse_parenthesized(),
            TokenKind::LeftBrace => {
                let open = self.token.span;
                self.parse_table_constructor()
                    .map_or(Expression::Error(open), Expression::TableConstructor)
            }
            TokenKind::Function => self.parse_function_def(),
            TokenKind::DotDotDot => self.parse_vararg(),
            // Luau
            TokenKind::Integer => Expression::Integer(self.advance_literal()),
            // Luau: `simpleexp ::= attributes 'function' funcbody`
            TokenKind::At if self.version.has_function_attributes() => self.parse_function_def(),
            // Luau
            TokenKind::If if self.version.has_if_exprs() => self.parse_if_expr(),
            // Luau
            TokenKind::InterpolatedStringBegin | TokenKind::InterpolatedStringSimple => {
                self.parse_interpolated_string()
            }
            _ => self.report_unexpected_in_expr(),
        }
    }

    fn parse_vararg(&mut self) -> Expression<'ast> {
        let span = self.advance();
        if !self.function_scope.is_vararg {
            self.error(span, "cannot use `...` outside a vararg function");
        }
        Expression::VarArg(span)
    }

    fn parse_parenthesized(&mut self) -> Expression<'ast> {
        let open = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(open);
        }
        self.advance();
        self.open(Closer::Paren);
        let expr = self.parse_expr(0);
        self.exit_depth();
        let close = self.expect_closer(TokenKind::RightParen, Closer::Paren);
        let span = open.merge(close);
        let parenthesized = Expression::new_parenthesized(span, expr, self.allocator);
        self.parse_suffixes(parenthesized)
    }

    #[cold]
    #[inline(never)]
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn report_unexpected_in_expr(&mut self) -> Expression<'ast> {
        let span = self.token.span;
        let hint = match self.peek() {
            TokenKind::End | TokenKind::Else | TokenKind::ElseIf | TokenKind::Until => {
                " (possible missing expression before keyword)"
            }
            TokenKind::RightParen => " (unmatched `)`)",
            TokenKind::RightBrace => " (unmatched `}`)",
            TokenKind::RightBracket => " (unmatched `]`)",
            TokenKind::Equal => " (use `==` for comparison, not `=`)",
            _ => "",
        };
        let message =
            format!("unexpected token {} in an expression{hint}", self.token.display(self.source));
        self.syntax_error(span, message);
        Expression::Error(span)
    }

    /// The suffixes after `prefix` loop into one node, so a run of any
    /// length is one level of nesting, counted toward the depth cap from the
    /// prefix's deepest level.
    ///
    /// Inlined apart from the run, so a prefix with no suffix makes no call,
    /// which measured 1% faster.
    #[inline]
    fn parse_suffixes(&mut self, prefix: Expression<'ast>) -> Expression<'ast> {
        if self.is_at_suffix() { self.parse_suffix_run(prefix) } else { prefix }
    }

    fn parse_suffix_run(&mut self, prefix: Expression<'ast>) -> Expression<'ast> {
        let level = self.nesting_depth;
        if !self.enter_wrapper(level) {
            return Expression::Error(prefix.span());
        }
        let start = self.scratch.suffixes.len();
        loop {
            self.parse_suffix();
            if !self.is_at_suffix() {
                break;
            }
        }
        self.nesting_depth = level;
        let suffixes = OneOrMore::split_off_in(&mut self.scratch.suffixes, start, self.allocator);
        let span = prefix.span().merge(suffixes.last().span());
        Expression::new_suffixed(span, prefix, suffixes, self.allocator)
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token ends the run")]
    #[inline]
    fn is_at_suffix(&self) -> bool {
        match self.peek() {
            TokenKind::Dot
            | TokenKind::LeftBracket
            | TokenKind::Colon
            | TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral => true,
            TokenKind::Less => self.is_at_type_instantiation(),
            _ => false,
        }
    }

    /// Parse one suffix onto the run being parsed.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other suffix is a call's arguments")]
    fn parse_suffix(&mut self) {
        let suffix = match self.peek() {
            TokenKind::Dot => {
                self.advance();
                Suffix::Field(self.expect_identifier_recover())
            }
            TokenKind::LeftBracket => {
                let open = self.advance();
                self.open(Closer::Bracket);
                let index = self.parse_expr(0);
                let close = self.expect_closer(TokenKind::RightBracket, Closer::Bracket);
                Suffix::Index { span: open.merge(close), index }
            }
            TokenKind::Colon => self.parse_method_call(),
            // Luau: explicit type-parameter instantiation is a suffix, so it
            // composes with field access, indexing, and calls.
            TokenKind::Less => {
                let open = self.token.span;
                let (type_args, close) = self.parse_type_instantiation();
                Suffix::TypeInstantiation {
                    span: open.merge(close),
                    type_args: self.alloc(type_args),
                }
            }
            _ => Suffix::Call(self.parse_function_args()),
        };
        self.scratch.suffixes.push(suffix);
    }

    fn parse_method_call(&mut self) -> Suffix<'ast> {
        self.advance();
        let name = self.expect_identifier_recover();
        let type_args = self.is_at_type_instantiation().then(|| {
            let (type_args, _) = self.parse_type_instantiation();
            self.alloc(type_args)
        });
        let args = self.parse_function_args();
        Suffix::new_method_call(name, type_args, args, self.allocator)
    }

    #[inline]
    fn is_at_type_instantiation(&self) -> bool {
        self.is_at(TokenKind::Less)
            && self.version.has_explicit_type_instantiation()
            && self.peek_next() == TokenKind::Less
    }

    /// Parse Luau's `<<T, U>>`, returning the type arguments and the span of
    /// the final `>`.
    fn parse_type_instantiation(&mut self) -> (TypeArgs<'ast>, Span) {
        self.advance(); // first `<`
        let type_args = self.parse_type_args(); // second `<` through first `>`
        let close = self.expect(TokenKind::Greater);
        (type_args, close)
    }

    fn parse_if_expr(&mut self) -> Expression<'ast> {
        let if_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(if_span);
        }
        self.advance();
        self.open(Closer::Else);
        let condition = self.parse_expr(0);
        self.expect(TokenKind::Then);
        let then_expr = self.parse_expr(0);
        let start = self.scratch.elseif_exprs.len();
        while self.is_at(TokenKind::ElseIf) {
            let elseif_span = self.advance();
            let clause_condition = self.parse_expr(0);
            self.expect(TokenKind::Then);
            let expr = self.parse_expr(0);
            let span = elseif_span.merge(expr.span());
            let clause = ElseIfExprClause::new(span, clause_condition, expr);
            self.scratch.elseif_exprs.push(clause);
        }
        self.close(Closer::Else);
        self.expect(TokenKind::Else);
        let else_expr = self.parse_expr(0);
        self.exit_depth();
        let span = if_span.merge(else_expr.span());
        let elseif_clauses =
            List::split_off_in(&mut self.scratch.elseif_exprs, start, self.allocator);
        Expression::new_if_expr(
            span,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
            self.allocator,
        )
    }

    /// The text of an interpolated string piece, between its delimiters.
    fn piece(&self, token: Token) -> Literal<'ast> {
        Literal::new(token.text(self.source), token.text_span())
    }

    fn parse_interpolated_string(&mut self) -> Expression<'ast> {
        let start_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(start_span);
        }
        let first = self.token;
        self.advance();
        if first.kind == TokenKind::InterpolatedStringSimple {
            self.exit_depth();
            let tail = self.piece(first);
            return Expression::new_interpolated_string(
                first.span,
                List::new(),
                tail,
                self.allocator,
            );
        }
        self.open(Closer::StringSegment);
        let start = self.scratch.segments.len();
        self.parse_interpolated_hole(first);
        while self.is_at(TokenKind::InterpolatedStringMiddle) {
            let middle = self.token;
            self.advance();
            self.parse_interpolated_hole(middle);
        }
        let (end_span, tail) = if self.is_at(TokenKind::InterpolatedStringEnd) {
            let end = self.token;
            self.advance();
            (end.span, self.piece(end))
        } else {
            let end_span = self.report_unfinished_interpolated_string();
            let at = self.previous_span.end;
            (end_span, Literal::new("", Span::new(at, at)))
        };
        self.close(Closer::StringSegment);
        self.exit_depth();
        let segments = List::split_off_in(&mut self.scratch.segments, start, self.allocator);
        let span = start_span.merge(end_span);
        Expression::new_interpolated_string(span, segments, tail, self.allocator)
    }

    /// The expression after `literal`, whose last byte is the `{` that opens
    /// it. A `{}` with none has its `}` at the current token.
    fn parse_interpolated_hole(&mut self, literal: Token) {
        let expr = if matches!(
            self.peek(),
            TokenKind::InterpolatedStringMiddle | TokenKind::InterpolatedStringEnd
        ) {
            self.report_empty_hole(literal)
        } else {
            self.parse_expr(0)
        };
        let text = self.piece(literal);
        self.scratch.segments.push(InterpolatedSegment::new(text, expr));
    }

    #[cold]
    #[inline(never)]
    fn report_empty_hole(&mut self, literal: Token) -> Expression<'ast> {
        let span =
            Span::new(literal.span.end.saturating_sub(1), self.token.span.start.saturating_add(1));
        self.error(span, "malformed interpolated string: expected expression inside `{}`");
        Expression::Error(span)
    }

    #[cold]
    #[inline(never)]
    fn report_unfinished_interpolated_string(&mut self) -> Span {
        let span = self.token.span;
        let message = format!(
            "expected interpolated string continuation, found {}",
            self.token.display(self.source)
        );
        self.syntax_error(span, message);
        span
    }

    /// The arguments sit one level under the run of suffixes, which
    /// [`Self::parse_suffix_run`] has already counted, so the parentheses add
    /// none of their own.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_function_args(&mut self) -> FunctionArgs<'ast> {
        match self.peek() {
            TokenKind::LeftParen => {
                // 5.1 and Luau reject a call whose `(` starts on a new line
                // ("ambiguous syntax: function call x new statement"); 5.2+
                // dropped the restriction.
                if self.token.has_line_break_before
                    && self.version.has_ambiguous_call_newline_error()
                {
                    self.report_ambiguous_call();
                }
                let open = self.advance();
                self.open(Closer::Paren);
                let args = if self.is_at(TokenKind::RightParen) {
                    List::new()
                } else {
                    self.parse_expr_list()
                };
                let close = self.expect_closer(TokenKind::RightParen, Closer::Paren);
                FunctionArgs::Parenthesized { span: open.merge(close), args }
            }
            TokenKind::LeftBrace => {
                let open = self.token.span;
                self.parse_table_constructor()
                    .map_or(FunctionArgs::Error(open), FunctionArgs::TableConstructor)
            }
            TokenKind::StringLiteral => FunctionArgs::StringLiteral(self.advance_leaf()),
            _ => self.report_missing_args(),
        }
    }

    #[cold]
    #[inline(never)]
    fn report_ambiguous_call(&mut self) {
        let span = self.token.span;
        self.error(
            span,
            "ambiguous syntax (function call x new statement): put `(` on the same line",
        );
    }

    #[cold]
    #[inline(never)]
    fn report_missing_args(&mut self) -> FunctionArgs<'ast> {
        let span = self.token.span;
        self.syntax_error(span, "expected function arguments");
        FunctionArgs::Error(span)
    }

    fn parse_function_def(&mut self) -> Expression<'ast> {
        let attributes = self.parse_function_attributes();
        let start_span = attributes.first().map_or(self.token.span, |attribute| attribute.span);
        if !self.is_at(TokenKind::Function) {
            return self.report_missing_function();
        }
        self.advance(); // `function`
        let Some(body) = self.parse_function_body() else {
            return Expression::Error(start_span);
        };
        let span = start_span.merge(body.span);
        Expression::new_function_def(span, attributes, body, self.allocator)
    }

    #[cold]
    #[inline(never)]
    fn report_missing_function(&mut self) -> Expression<'ast> {
        let span = self.token.span;
        self.syntax_error(span, "expected `function` after attribute");
        Expression::Error(span)
    }

    /// `None` when the table would pass the depth cap.
    pub(super) fn parse_table_constructor(&mut self) -> Option<Box<'ast, TableConstructor<'ast>>> {
        if !self.enter_depth() {
            return None;
        }
        let open = self.advance(); // `{`
        self.open(Closer::Brace);

        // Every Lua version rejects a leading separator (`{;}`, `{, 1}`), and
        // so does Luau. Consume it so parsing recovers.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon) {
            self.report_leading_separator();
        }

        let start = self.scratch.fields.len();
        let mut has_trailing_separator = false;
        while !matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
            self.parse_field();
            has_trailing_separator = matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon);
            if !has_trailing_separator {
                break;
            }
            self.advance();
        }
        self.exit_depth();

        let close = self.expect_closer(TokenKind::RightBrace, Closer::Brace);
        let fields = List::split_off_in(&mut self.scratch.fields, start, self.allocator);
        let span = open.merge(close);
        Some(TableConstructor::boxed(span, fields, has_trailing_separator, self.allocator))
    }

    #[cold]
    #[inline(never)]
    fn report_leading_separator(&mut self) {
        let span = self.advance();
        self.error(span, "unexpected separator in table constructor");
    }

    /// Parse one field onto the table being parsed.
    fn parse_field(&mut self) {
        let field = if self.is_at(TokenKind::LeftBracket) {
            // `[expr] = expr`
            let open = self.advance();
            self.open(Closer::Bracket);
            let key = self.parse_expr(0);
            self.expect_closer(TokenKind::RightBracket, Closer::Bracket);
            self.expect(TokenKind::Equal);
            let value = self.parse_expr(0);
            let span = open.merge(value.span());
            Field::Bracketed { span, key, value }
        } else if self.is_at_identifier() && self.peek_next() == TokenKind::Equal {
            // `Name = expr`, which needs one token of lookahead for the `=`.
            let name = self.advance_name();
            self.advance();
            let value = self.parse_expr(0);
            Field::Named { name, value }
        } else {
            Field::Positional(self.parse_expr(0))
        };
        self.scratch.fields.push(field);
    }
}
