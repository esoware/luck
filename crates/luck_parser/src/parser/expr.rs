use luck_allocator::List;
use luck_ast::{
    BinaryLink, BinaryOp, ElseIfExprClause, Expression, Field, FunctionArgs, FunctionDef,
    IfExpression, InterpolatedSegment, InterpolatedString, Literal, MethodCall, OneOrMore,
    ParenthesizedExpression, Suffix, SuffixedExpression, TableConstructor, TypeArgs, TypeCast,
    UnaryOp,
};
use luck_token::{BinOp, Span, Token, TokenKind, UnOp};

use super::{Closer, Parser, empty_body};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl<'ast> Parser<'ast> {
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression<'ast> {
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let mut expr = self.parse_prefix();
        if let Some(op) = self.peek_binary_op()
            && op.precedence().0 >= min_precedence
        {
            self.parse_binary_runs(&mut expr, op, min_precedence, level);
        }
        self.end_operand(outer);
        expr
    }

    /// Each precedence level's operators run in a loop into one node, so a
    /// run of any length is one level of nesting, counted toward the depth
    /// cap from its first operand's deepest level. `expr` is that operand,
    /// and becomes the runs.
    #[inline(never)]
    fn parse_binary_runs(
        &mut self,
        expr: &mut Expression<'ast>,
        first_op: BinOp,
        min_precedence: u8,
        level: usize,
    ) {
        let mut op = first_op;
        loop {
            if !self.enter_wrapper(level) {
                *expr = Expression::Error(expr.span());
                return;
            }
            self.parse_binary_run(expr, op);
            self.nesting_depth = level;
            match self.peek_binary_op() {
                Some(next_op) if next_op.precedence().0 >= min_precedence => op = next_op,
                Some(_) | None => return,
            }
        }
    }

    /// The run's operands bind tighter, so a right-associative run loops like
    /// a left-associative one and only the grouping differs. `expr` holds the
    /// first operand until the run is whole.
    #[inline]
    fn parse_binary_run(&mut self, expr: &mut Expression<'ast>, op: BinOp) {
        self.advance();
        let precedence = op.precedence().0;
        let operand_precedence = precedence.saturating_add(1);
        let start = self.scratch.binary_links.len();
        let operand = self.parse_expr(operand_precedence);
        self.scratch.binary_links.push(BinaryLink { op, operand });
        while let Some(next_op) =
            self.peek_binary_op().filter(|next_op| next_op.precedence().0 == precedence)
        {
            self.advance();
            let next_operand = self.parse_expr(operand_precedence);
            self.scratch.binary_links.push(BinaryLink { op: next_op, operand: next_operand });
        }
        self.finish_binary_run(expr, start);
    }

    /// Out of line, so the run's recursive frame holds none of this.
    #[inline(never)]
    fn finish_binary_run(&mut self, expr: &mut Expression<'ast>, start: usize) {
        let links = OneOrMore::split_off_in(&mut self.scratch.binary_links, start, self.allocator);
        let first = expr.take();
        let span = first.span().merge(links.last().operand.span());
        *expr = Expression::BinaryOp(self.alloc(BinaryOp { span, first, links }));
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
        self.parse_exprs_after(first)
    }

    /// `first` and the expression after each comma that follows it.
    fn parse_exprs_after(&mut self, first: Expression<'ast>) -> List<'ast, Expression<'ast>> {
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
        let mut expr = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        if self.is_at(TokenKind::DoubleColon) && self.version.has_type_syntax() {
            self.parse_type_cast(&mut expr, level);
        }
        expr
    }

    /// Unary chains like `not not not x` recurse once per operator, so they
    /// count toward the depth limit too.
    #[inline(never)]
    fn parse_unary(&mut self, op: UnOp) -> Expression<'ast> {
        let op_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(op_span);
        }
        self.advance();
        let operand = self.parse_expr(UnOp::PRECEDENCE);
        self.exit_depth();
        let span = op_span.merge(operand.span());
        Expression::UnaryOp(self.alloc(UnaryOp { span, op, operand }))
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    #[inline(never)]
    fn parse_type_cast(&mut self, expr: &mut Expression<'ast>, level: usize) {
        if !self.enter_wrapper(level) {
            *expr = Expression::Error(expr.span());
            return;
        }
        self.advance();
        let ty = self.parse_type();
        self.nesting_depth = level;
        let operand = expr.take();
        let span = operand.span().merge(ty.span());
        *expr = Expression::TypeCast(self.alloc(TypeCast { span, expr: operand, ty }));
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    #[inline]
    fn parse_primary_expr(&mut self) -> Expression<'ast> {
        match self.peek() {
            TokenKind::Identifier => {
                let mut expr = Expression::Name(self.advance_boxed_name());
                if self.is_at_suffix() {
                    self.parse_suffix_run(&mut expr);
                }
                expr
            }
            TokenKind::Number => Expression::Number(self.advance_literal()),
            TokenKind::StringLiteral => Expression::StringLiteral(self.advance_literal()),
            TokenKind::Nil => Expression::Nil(self.advance()),
            TokenKind::False => Expression::False(self.advance()),
            TokenKind::True => Expression::True(self.advance()),
            TokenKind::LeftParen => self.parse_parenthesized(),
            TokenKind::LeftBrace => self.parse_table_constructor(),
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

    #[inline(never)]
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
        let mut parenthesized =
            Expression::Parenthesized(self.alloc(ParenthesizedExpression { span, expr }));
        if self.is_at_suffix() {
            self.parse_suffix_run(&mut parenthesized);
        }
        parenthesized
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

    /// The suffixes after `expr` loop into one node, so a run of any length
    /// is one level of nesting, counted toward the depth cap from the
    /// prefix's deepest level. `expr` holds the prefix until the run is
    /// whole.
    #[inline(never)]
    fn parse_suffix_run(&mut self, expr: &mut Expression<'ast>) {
        let level = self.nesting_depth;
        if !self.enter_wrapper(level) {
            *expr = Expression::Error(expr.span());
            return;
        }
        let start = self.scratch.suffixes.len();
        loop {
            self.parse_suffix();
            if !self.is_at_suffix() {
                break;
            }
        }
        self.nesting_depth = level;
        self.finish_suffix_run(expr, start);
    }

    /// Out of line, so the run's recursive frame holds none of this.
    #[inline(never)]
    fn finish_suffix_run(&mut self, expr: &mut Expression<'ast>, start: usize) {
        let suffixes = OneOrMore::split_off_in(&mut self.scratch.suffixes, start, self.allocator);
        let prefix = expr.take();
        let span = prefix.span().merge(suffixes.last().span());
        *expr = Expression::Suffixed(self.alloc(SuffixedExpression { span, prefix, suffixes }));
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

    /// Parse one suffix onto the run being parsed. Each kind that recurses
    /// is out of line, so the run's frame holds none of its locals.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other suffix is a call's arguments")]
    #[inline]
    fn parse_suffix(&mut self) {
        match self.peek() {
            TokenKind::Dot => {
                self.advance();
                let name = self.expect_identifier_recover();
                self.scratch.suffixes.push(Suffix::Field(name));
            }
            TokenKind::LeftBracket => self.parse_index_suffix(),
            TokenKind::Colon => self.parse_method_call(),
            // Luau: explicit type-parameter instantiation is a suffix, so it
            // composes with field access, indexing, and calls.
            TokenKind::Less => self.parse_type_instantiation_suffix(),
            _ => self.parse_call_suffix(),
        }
    }

    #[inline(never)]
    fn parse_index_suffix(&mut self) {
        let open = self.advance();
        self.open(Closer::Bracket);
        let index = self.parse_expr(0);
        let close = self.expect_closer(TokenKind::RightBracket, Closer::Bracket);
        self.scratch.suffixes.push(Suffix::Index { span: open.merge(close), index });
    }

    #[inline(never)]
    fn parse_call_suffix(&mut self) {
        let args = self.parse_function_args();
        self.scratch.suffixes.push(Suffix::Call(args));
    }

    #[inline(never)]
    fn parse_type_instantiation_suffix(&mut self) {
        let open = self.token.span;
        let (args, close) = self.parse_type_instantiation();
        let suffix =
            Suffix::TypeInstantiation { span: open.merge(close), type_args: self.alloc(args) };
        self.scratch.suffixes.push(suffix);
    }

    #[inline(never)]
    fn parse_method_call(&mut self) {
        self.advance();
        let name = self.expect_identifier_recover();
        let type_args = self.is_at_type_instantiation().then(|| {
            let (type_args, _) = self.parse_type_instantiation();
            self.alloc(type_args)
        });
        let args = self.parse_function_args();
        let call = self.alloc(MethodCall { name, type_args, args });
        self.scratch.suffixes.push(Suffix::MethodCall(call));
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

    #[inline(never)]
    fn parse_if_expr(&mut self) -> Expression<'ast> {
        let if_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(if_span);
        }
        self.advance();
        self.open(Closer::Else);
        let mut node = self.alloc(IfExpression {
            span: if_span,
            condition: Expression::Error(if_span),
            then_expr: Expression::Error(if_span),
            elseif_clauses: List::new(),
            else_expr: Expression::Error(if_span),
        });
        node.condition = self.parse_expr(0);
        self.expect(TokenKind::Then);
        node.then_expr = self.parse_expr(0);
        let start = self.scratch.elseif_exprs.len();
        while self.is_at(TokenKind::ElseIf) {
            let elseif_span = self.advance();
            let clause_condition = self.parse_expr(0);
            self.expect(TokenKind::Then);
            let expr = self.parse_expr(0);
            let span = elseif_span.merge(expr.span());
            let clause = ElseIfExprClause { span, condition: clause_condition, expr };
            self.scratch.elseif_exprs.push(clause);
        }
        node.elseif_clauses =
            List::split_off_in(&mut self.scratch.elseif_exprs, start, self.allocator);
        self.close(Closer::Else);
        self.expect(TokenKind::Else);
        node.else_expr = self.parse_expr(0);
        self.exit_depth();
        node.span = if_span.merge(node.else_expr.span());
        Expression::IfExpression(node)
    }

    /// The text of an interpolated string piece, between its delimiters.
    fn piece(&self, token: Token) -> Literal<'ast> {
        Literal { text: token.text(self.source), span: token.text_span() }
    }

    #[inline(never)]
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
            let string = InterpolatedString { span: first.span, segments: List::new(), tail };
            return Expression::InterpolatedString(self.alloc(string));
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
            (end_span, Literal { text: "", span: Span::new(at, at) })
        };
        self.close(Closer::StringSegment);
        self.exit_depth();
        let segments = List::split_off_in(&mut self.scratch.segments, start, self.allocator);
        let span = start_span.merge(end_span);
        Expression::InterpolatedString(self.alloc(InterpolatedString { span, segments, tail }))
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
        self.scratch.segments.push(InterpolatedSegment { text, expr });
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
    #[expect(clippy::inline_always, reason = "measured: a frame of its own costs stack per level")]
    #[inline(always)]
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
                let table = self.parse_table_constructor();
                if let Expression::TableConstructor(constructor) = table {
                    FunctionArgs::TableConstructor(constructor)
                } else {
                    FunctionArgs::Error(table.span())
                }
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

    #[inline(never)]
    fn parse_function_def(&mut self) -> Expression<'ast> {
        let attributes = self.parse_function_attributes();
        let start_span = attributes.first().map_or(self.token.span, |attribute| attribute.span);
        if !self.is_at(TokenKind::Function) {
            return self.report_missing_function();
        }
        self.advance(); // `function`
        let body = empty_body(start_span);
        let mut node = self.alloc(FunctionDef { span: start_span, attributes, body });
        if !self.parse_function_body(&mut node.body) {
            return Expression::Error(start_span);
        }
        node.span = start_span.merge(node.body.span);
        Expression::FunctionDef(node)
    }

    #[cold]
    #[inline(never)]
    fn report_missing_function(&mut self) -> Expression<'ast> {
        let span = self.token.span;
        self.syntax_error(span, "expected `function` after attribute");
        Expression::Error(span)
    }

    /// An error at the `{` when the table would pass the depth cap.
    #[inline(never)]
    pub(super) fn parse_table_constructor(&mut self) -> Expression<'ast> {
        let open = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(open);
        }
        self.advance(); // `{`
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
        let table = TableConstructor { span: open.merge(close), fields, has_trailing_separator };
        Expression::TableConstructor(self.alloc(table))
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
