use luck_ast::{
    BinaryLink, BinaryOp, CallSuffix, ElseIfExprClause, Expression, Field, FunctionArgs,
    FunctionDef, IfExpression, InterpolatedString, InterpolatedStringSegment, OneOrMore,
    ParenthesizedExpression, Punctuated, Suffix, SuffixedExpression, TableConstructor, TypeArgs,
    TypeCast, UnaryOp,
};
use luck_token::{BinOp, Span, Token, TokenKind, UnOp};

use super::Parser;
use super::skip::{ExprTail, TypeExtent};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// Each precedence level's operators run in a loop into one node, so a
    /// run of any length is one level of nesting, counted toward the depth
    /// cap from its first operand's deepest level.
    #[inline]
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression {
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let mut lhs = self.parse_prefix();

        while let Some(op) = self.peek_binary_op() {
            let precedence = op.precedence().0;
            if precedence < min_precedence {
                break;
            }
            if !self.enter_wrapper(level) {
                lhs = self.refuse_binary_run(lhs.span(), precedence, level);
                continue;
            }
            self.advance_span();
            lhs = self.parse_binary_run(lhs, op, precedence);
            self.nesting_depth = level;
        }

        self.end_operand(outer);
        lhs
    }

    /// Refuse the run at `precedence` that the current operator would start
    /// over the operand from `start`.
    #[cold]
    #[inline(never)]
    fn refuse_binary_run(&mut self, start: Span, precedence: u8, level: usize) -> Expression {
        self.refuse_wrapper(level);
        self.skip_expr_tail(ExprTail::Operators(precedence));
        Expression::Error(start.merge(self.previous_span))
    }

    /// Refuse the expression at the current token, skipping it with `tail`.
    #[cold]
    #[inline(never)]
    fn refuse_expr(&mut self, tail: ExprTail) -> Expression {
        let start = self.current_span();
        self.report_depth_limit(start);
        self.skip_expr(tail);
        Expression::Error(start.merge(self.previous_span))
    }

    pub(super) fn peek_binary_op(&self) -> Option<BinOp> {
        let op = BinOp::from_token_kind(self.peek())?;
        // Luau's lexer emits `&` and `|` for the type grammar only; they
        // aren't binary operators there.
        (!op.is_bitwise() || self.version.has_bitwise_ops()).then_some(op)
    }

    /// The run of operators at `precedence` that `first` and the consumed
    /// `op` start. Its operands bind tighter, so a right-associative run
    /// loops like a left-associative one and only the grouping differs.
    fn parse_binary_run(&mut self, first: Expression, op: BinOp, precedence: u8) -> Expression {
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
        let span = first.span().merge(links.last().operand.span());
        Expression::BinaryOp(Box::new(BinaryOp { span, first, links }))
    }

    pub(super) fn parse_expr_list(&mut self) -> Punctuated<Expression> {
        let mut exprs = vec![self.parse_expr(0)];

        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr(0));
        }

        Punctuated::from_items(exprs)
    }

    // The recursive functions here each keep one construct's temporaries, so
    // a level of nesting costs an unoptimized build only the frames of the
    // construct it nests through.
    fn parse_prefix(&mut self) -> Expression {
        if let Some(op) = UnOp::from_token_kind(self.peek()) {
            return self.parse_unary_op(op);
        }
        let level = self.nesting_depth;
        let primary = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        self.parse_type_cast(primary, level)
    }

    fn parse_unary_op(&mut self, op: UnOp) -> Expression {
        // Unary chains like `not not not x` recurse once per operator, so
        // they count toward the depth limit too.
        if !self.can_nest(1) {
            return self.refuse_expr(ExprTail::Operators(UnOp::PRECEDENCE));
        }
        let op_span = self.advance_span();
        self.enter_depth();
        let operand = self.parse_expr(UnOp::PRECEDENCE);
        self.exit_depth();
        let span = op_span.merge(operand.span());
        Expression::UnaryOp(Box::new(UnaryOp { span, op, operand }))
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    fn parse_type_cast(&mut self, expr: Expression, level: usize) -> Expression {
        if self.version.has_type_syntax() && matches!(self.peek(), TokenKind::DoubleColon) {
            if !self.enter_wrapper(level) {
                return self.refuse_type_cast(expr.span(), level);
            }
            self.advance_span();
            let type_annotation = self.parse_type();
            self.nesting_depth = level;
            let span = expr.span().merge(type_annotation.span());
            return Expression::TypeCast(Box::new(TypeCast { span, expr, type_annotation }));
        }
        expr
    }

    #[cold]
    #[inline(never)]
    fn refuse_type_cast(&mut self, start: Span, level: usize) -> Expression {
        self.refuse_wrapper(level);
        self.advance();
        self.skip_type(TypeExtent::Full);
        Expression::Error(start.merge(self.previous_span))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_primary_expr(&mut self) -> Expression {
        match self.peek() {
            TokenKind::Nil => Expression::Nil(self.advance_span()),
            TokenKind::False => Expression::False(self.advance_span()),
            TokenKind::True => Expression::True(self.advance_span()),
            TokenKind::Number(_) => self.parse_number(),
            TokenKind::StringLiteral(_) => Expression::StringLiteral(self.advance_literal()),
            TokenKind::DotDotDot => self.parse_vararg(),
            TokenKind::Function => self.parse_function_def(),
            // Luau: `simpleexp ::= attributes 'function' funcbody`
            TokenKind::At if self.version.has_function_attributes() => self.parse_function_def(),
            TokenKind::LeftBrace => self.parse_table_expr(),
            TokenKind::LeftParen => {
                let parenthesized = self.parse_parenthesized();
                self.parse_suffixes(parenthesized)
            }
            // Luau if-expression.
            TokenKind::If if self.version.has_if_exprs() => self.parse_if_expr(),
            // Luau interpolated string.
            TokenKind::InterpolatedStringBegin(_) => self.parse_interpolated_string(),
            TokenKind::Identifier(_) => {
                let name = self.parse_name();
                self.parse_suffixes(name)
            }
            _ => self.report_unexpected_in_expr(),
        }
    }

    fn parse_vararg(&mut self) -> Expression {
        let span = self.advance_span();
        if !self.function_scope.is_vararg {
            self.error(span, "cannot use `...` outside a vararg function");
        }
        Expression::VarArg(span)
    }

    fn parse_table_expr(&mut self) -> Expression {
        if !self.can_nest(1) {
            return Expression::Error(self.refuse_group());
        }
        Expression::TableConstructor(Box::new(self.parse_table_constructor()))
    }

    fn parse_parenthesized(&mut self) -> Expression {
        if !self.can_nest(1) {
            return Expression::Error(self.refuse_group());
        }
        let open = self.advance_span();
        self.enter_depth();
        let expr = self.parse_expr(0);
        self.exit_depth();
        let close = self.expect(&TokenKind::RightParen);
        let span = open.merge(close);
        Expression::Parenthesized(Box::new(ParenthesizedExpression { span, expr }))
    }

    fn parse_name(&mut self) -> Expression {
        Expression::Name(self.advance())
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
        self.error(span, format!("unexpected token {kind} in an expression{hint}"));
        Expression::Error(span)
    }

    fn parse_number(&mut self) -> Expression {
        let literal = self.advance_literal();
        if literal.text.ends_with('i') {
            Expression::Integer(literal)
        } else {
            Expression::Number(literal)
        }
    }

    /// The suffixes after `prefix` loop into one node, so a run of any
    /// length is one level of nesting, counted toward the depth cap from the
    /// prefix's deepest level.
    fn parse_suffixes(&mut self, prefix: Expression) -> Expression {
        let level = self.nesting_depth;
        if !self.is_at_suffix() {
            return prefix;
        }
        if !self.enter_wrapper(level) {
            return self.refuse_suffixes(prefix.span(), level);
        }
        let first = self.parse_suffix();
        let start = self.suffixes.len();
        while self.is_at_suffix() {
            let suffix = self.parse_suffix();
            self.suffixes.push(suffix);
        }
        self.nesting_depth = level;
        let suffixes = OneOrMore::from_first(first, self.suffixes.drain(start..));
        let span = prefix.span().merge(suffixes.last().span());
        Expression::Suffixed(Box::new(SuffixedExpression { span, prefix, suffixes }))
    }

    #[cold]
    #[inline(never)]
    fn refuse_suffixes(&mut self, start: Span, level: usize) -> Expression {
        self.refuse_wrapper(level);
        self.skip_suffixes();
        Expression::Error(start.merge(self.previous_span))
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token ends the run")]
    pub(super) const fn is_at_suffix(&self) -> bool {
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
                Suffix::TypeInstantiation { span: open.merge(close), type_args }
            }
            TokenKind::Dot => {
                self.advance_span();
                Suffix::Field(self.expect_identifier_recover())
            }
            TokenKind::LeftBracket => {
                let open = self.advance_span();
                let index = self.parse_expr(0);
                let close = self.expect(&TokenKind::RightBracket);
                Suffix::Index { span: open.merge(close), index }
            }
            TokenKind::Colon => self.parse_method_suffix(),
            _ => Suffix::Call(CallSuffix {
                method: None,
                explicit_type_args: None,
                args: self.parse_function_args(),
            }),
        }
    }

    fn parse_method_suffix(&mut self) -> Suffix {
        self.advance_span();
        let method = Some(self.expect_identifier_recover());
        let explicit_type_args =
            self.is_at_type_instantiation().then(|| Box::new(self.parse_type_instantiation().0));
        let args = self.parse_function_args();
        Suffix::Call(CallSuffix { method, explicit_type_args, args })
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
        if !self.can_nest(1) {
            return self.refuse_expr(ExprTail::Nothing);
        }
        let if_span = self.advance_span();
        self.enter_depth();
        let condition = self.parse_expr(0);
        self.expect(&TokenKind::Then);
        let then_expr = self.parse_expr(0);

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            elseif_clauses.push(self.parse_elseif_expr_clause());
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

    fn parse_elseif_expr_clause(&mut self) -> ElseIfExprClause {
        let elseif_span = self.advance_span();
        let condition = self.parse_expr(0);
        self.expect(&TokenKind::Then);
        let expr = self.parse_expr(0);
        ElseIfExprClause { span: elseif_span.merge(expr.span()), condition, expr }
    }

    fn parse_interpolated_string(&mut self) -> Expression {
        if !self.can_nest(1) {
            return Expression::Error(self.refuse_group());
        }
        let begin_token = self.advance();
        let start_span = begin_token.span;
        let mut segments = Vec::new();
        self.enter_depth();
        let interpolated = self.parse_interpolated_segments(begin_token, &mut segments);
        self.exit_depth();
        let span = start_span.merge(interpolated);
        Expression::InterpolatedString(Box::new(InterpolatedString { span, segments }))
    }

    /// Fills `segments` and returns the span of the last token read.
    fn parse_interpolated_segments(
        &mut self,
        begin_token: Token,
        segments: &mut Vec<InterpolatedStringSegment>,
    ) -> Span {
        if matches!(self.peek(), TokenKind::InterpolatedStringEnd(_)) {
            return self.parse_holeless_interpolated_string(begin_token, segments);
        }
        self.parse_interpolated_hole(begin_token, segments);
        while matches!(self.peek(), TokenKind::InterpolatedStringMiddle(_)) {
            let middle_token = self.advance();
            self.parse_interpolated_hole(middle_token, segments);
        }
        if !matches!(self.peek(), TokenKind::InterpolatedStringEnd(_)) {
            return self.report_unfinished_interpolated_string();
        }
        let end_token = self.advance();
        let span = end_token.span;
        segments.push(InterpolatedStringSegment { literal: end_token, expr: None });
        span
    }

    fn parse_interpolated_hole(
        &mut self,
        literal: Token,
        segments: &mut Vec<InterpolatedStringSegment>,
    ) {
        let expr = Some(self.parse_expr(0));
        segments.push(InterpolatedStringSegment { literal, expr });
    }

    fn parse_holeless_interpolated_string(
        &mut self,
        begin_token: Token,
        segments: &mut Vec<InterpolatedStringSegment>,
    ) -> Span {
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
        let span = end_token.span;
        segments.push(InterpolatedStringSegment { literal: end_token, expr: None });
        span
    }

    #[cold]
    #[inline(never)]
    fn report_unfinished_interpolated_string(&mut self) -> Span {
        let span = self.current_span();
        self.error(
            span,
            format!("expected interpolated string continuation, found {}", self.peek()),
        );
        span
    }

    /// The arguments sit one level under the run of suffixes, which
    /// [`Self::parse_suffixes`] has already counted, so the parentheses add
    /// none of their own.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_function_args(&mut self) -> FunctionArgs {
        match self.peek() {
            TokenKind::LeftParen => self.parse_parenthesized_args(),
            // `FunctionArgs` has no error node of its own, so a refused table
            // stands as an error argument.
            TokenKind::LeftBrace if !self.can_nest(1) => {
                let span = self.refuse_group();
                FunctionArgs::Parenthesized {
                    span,
                    args: Punctuated::from_item(Expression::Error(span)),
                }
            }
            TokenKind::LeftBrace => {
                FunctionArgs::TableConstructor(Box::new(self.parse_table_constructor()))
            }
            TokenKind::StringLiteral(_) => FunctionArgs::StringLiteral(self.advance_literal()),
            _ => {
                let span = self.current_span();
                self.error(span, "expected function arguments");
                FunctionArgs::Parenthesized { span, args: Punctuated::empty() }
            }
        }
    }

    fn parse_parenthesized_args(&mut self) -> FunctionArgs {
        // 5.1 and Luau reject a call whose `(` starts on a new line
        // ("ambiguous syntax: function call x new statement"); 5.2+
        // dropped the restriction.
        if self.version.has_ambiguous_call_newline_error() && self.has_newline_before_current() {
            let span = self.current_span();
            self.error(
                span,
                "ambiguous syntax (function call x new statement): put `(` on the same line",
            );
        }
        let open = self.advance_span();
        let args = if matches!(self.peek(), TokenKind::RightParen) {
            Punctuated::empty()
        } else {
            self.parse_expr_list()
        };
        let close = self.expect(&TokenKind::RightParen);
        FunctionArgs::Parenthesized { span: open.merge(close), args }
    }

    fn parse_function_def(&mut self) -> Expression {
        if !self.can_nest_function() {
            return self.refuse_expr(ExprTail::Nothing);
        }
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

    /// The caller has checked that the table fits.
    pub(super) fn parse_table_constructor(&mut self) -> TableConstructor {
        let start_span = self.advance_span(); // `{`
        self.enter_depth();

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
            return self.parse_bracketed_field();
        }

        // `Name = expr`, which needs one token of lookahead for the `=`.
        if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Equal) {
            return self.parse_named_field();
        }

        let value = self.parse_expr(0);
        let span = value.span();
        Field::Positional { span, value }
    }

    fn parse_bracketed_field(&mut self) -> Field {
        let open = self.advance_span();
        let key = self.parse_expr(0);
        self.expect(&TokenKind::RightBracket);
        self.expect(&TokenKind::Equal);
        let value = self.parse_expr(0);
        let span = open.merge(value.span());
        Field::Bracketed { span, key, value }
    }

    fn parse_named_field(&mut self) -> Field {
        let name = self.advance();
        self.advance_span();
        let value = self.parse_expr(0);
        let span = name.span.merge(value.span());
        Field::Named { span, name, value }
    }
}
