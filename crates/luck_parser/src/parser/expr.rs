use luck_allocator::{Box, List, ThinBytes};
use luck_ast::{
    BinaryLink, ElseIfExprClause, Expression, Field, FunctionArgs, GetSpan as _,
    InterpolatedSegment, OneOrMore, StringLiteral, Suffix, SuffixedExpression, TableConstructor,
    TypeArgs,
};
use luck_token::{BinOp, Span, Token, TokenKind, UnOp};

use super::recovery::Closer;
use super::{Parser, name_token};

/// Whether `kind` can open a suffix, which `<` does only as the first of the
/// `<<` that opens a type instantiation.
const fn can_open_suffix(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Dot
            | TokenKind::LeftBracket
            | TokenKind::Colon
            | TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral
            | TokenKind::Less
    )
}

/// Whether `kind` after a name or a literal ends the expression there: it
/// opens no suffix, no cast, and no binary operator.
const fn ends_operand(kind: TokenKind) -> bool {
    !can_open_suffix(kind)
        && !matches!(kind, TokenKind::DoubleColon)
        && BinOp::from_token_kind(kind).is_none()
}

/// The tokens [`Parser::parse_lone_operand`] reads as a whole operand.
const fn is_lone_operand(kind: TokenKind) -> bool {
    matches!(
        kind,
        name_token!(
            TokenKind::Number
                | TokenKind::StringLiteral
                | TokenKind::Nil
                | TokenKind::False
                | TokenKind::True
                | TokenKind::DotDotDot
        )
    )
}

impl<'ast> Parser<'ast> {
    /// Most expressions are a name or a literal alone, which the token after
    /// it shows, so they skip the recursion through the whole grammar.
    ///
    /// Out of line, so the check is one copy rather than one in each of the
    /// grammar's calls, which keeps its code and its recursive frames small.
    #[inline(never)]
    pub(super) fn parse_expr(&mut self, min_precedence: u8) -> Expression<'ast> {
        if is_lone_operand(self.peek()) && ends_operand(self.peek_next()) {
            return self.parse_lone_operand(min_precedence);
        }
        self.parse_compound_expr(min_precedence)
    }

    /// The current token as the whole expression, which it is when it's a
    /// name or a literal and the token after it ends it, as
    /// [`Self::parse_compound_expr`] would read it. It opens no level of
    /// nesting, so the deepest level stays where it was.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the full grammar")]
    fn parse_lone_operand(&mut self, min_precedence: u8) -> Expression<'ast> {
        match self.peek() {
            name_token!() => self.parse_name_expr(),
            TokenKind::Number => Expression::Number(self.advance_number()),
            TokenKind::StringLiteral => Expression::StringLiteral(self.advance_string()),
            TokenKind::Nil => Expression::Nil(self.advance()),
            TokenKind::False => Expression::False(self.advance()),
            TokenKind::True => Expression::True(self.advance()),
            TokenKind::DotDotDot => self.parse_vararg(),
            _ => self.parse_compound_expr(min_precedence),
        }
    }

    /// Each precedence level's operators run in a loop into one node, so a
    /// run of any length is one level of nesting, counted toward the depth
    /// cap from its first operand's deepest level.
    fn parse_compound_expr(&mut self, min_precedence: u8) -> Expression<'ast> {
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let start = self.token.span.start;
        let mut lhs = self.parse_prefix();
        while let Some(op) = BinOp::from_token_kind(self.peek()) {
            let precedence = op.precedence().0;
            if precedence < min_precedence {
                break;
            }
            if !self.enter_wrapper(level) {
                lhs = Expression::Error(lhs.span());
                break;
            }
            lhs = self.parse_binary_run(lhs, op, precedence, start);
            self.nesting_depth = level;
        }
        self.end_operand(outer);
        lhs
    }

    /// The run's operands bind tighter, so a right-associative run loops like
    /// a left-associative one and only the grouping differs.
    ///
    /// Parentheses end a run, so an operand in them stays a run of its own.
    /// Most runs hold one operator, whose link goes straight to the arena.
    fn parse_binary_run(
        &mut self,
        first: Expression<'ast>,
        op: BinOp,
        precedence: u8,
        start: u32,
    ) -> Expression<'ast> {
        self.advance();
        let operand_precedence = precedence + 1;
        let operand = self.parse_expr(operand_precedence);
        let first_link = BinaryLink::new(op, operand);
        let links = if self.operator_at(precedence).is_some() {
            let links_start = self.scratch.binary_links.len();
            self.scratch.binary_links.push(first_link);
            while let Some(next_op) = self.operator_at(precedence) {
                self.advance();
                let next_operand = self.parse_expr(operand_precedence);
                self.scratch.binary_links.push(BinaryLink::new(next_op, next_operand));
            }
            OneOrMore::split_off_in(&mut self.scratch.binary_links, links_start, self.allocator)
        } else {
            OneOrMore::new(first_link, self.allocator)
        };
        let span = Span::new(start, self.previous_end);
        Expression::new_binary_op(span, first, links, self.allocator)
    }

    /// The current token's binary operator, if it has one at `precedence`.
    #[inline]
    fn operator_at(&self, precedence: u8) -> Option<BinOp> {
        BinOp::from_token_kind(self.peek()).filter(|op| op.precedence().0 == precedence)
    }

    /// Most lists hold one or two expressions, which move to the arena as
    /// values. The scratch holds the first while the second parses, so it
    /// takes no room in the frame of every level of nesting in between.
    pub(super) fn parse_expr_list(&mut self) -> List<'ast, Expression<'ast>> {
        let first = self.parse_expr(0);
        if !self.is_at(TokenKind::Comma) {
            return List::from_array_in([first], self.allocator);
        }
        let start = self.scratch.exprs.len();
        self.scratch.exprs.push(first);
        self.advance();
        let second = self.parse_expr(0);
        if !self.is_at(TokenKind::Comma) {
            return self.pair_with_held(second);
        }
        self.scratch.exprs.push(second);
        while self.eat(TokenKind::Comma) {
            let expr = self.parse_expr(0);
            self.scratch.exprs.push(expr);
        }
        List::split_off_in(&mut self.scratch.exprs, start, self.allocator)
    }

    /// The list of the expression the scratch holds last and `second`.
    /// Apart from [`Self::parse_expr_list`], so the pair it builds takes no
    /// room in the frame that every level of nesting in a call takes.
    #[inline(never)]
    fn pair_with_held(&mut self, second: Expression<'ast>) -> List<'ast, Expression<'ast>> {
        let held = self.scratch.exprs.pop().expect("the list's first expression is held");
        List::from_array_in([held, second], self.allocator)
    }

    #[inline]
    fn parse_prefix(&mut self) -> Expression<'ast> {
        if let Some(op) = UnOp::from_token_kind(self.peek()) {
            return self.parse_unary(op);
        }
        let level = self.nesting_depth;
        let start = self.token.span.start;
        let primary = self.parse_primary_expr();
        // Casts apply to any simpleexp (`1 :: number`, `{} :: Foo`), not just
        // suffix chains, so they're parsed here.
        if self.is_at(TokenKind::DoubleColon) {
            return self.parse_type_cast(primary, level, start);
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
        let span = Span::new(op_span.start, self.previous_end);
        Expression::new_unary_op(span, op, operand, self.allocator)
    }

    /// Luau's grammar is `asexp ::= simpleexp ['::' Type]`, so `x :: A :: B`
    /// is a parse error there. A second `::` is left in place to fail as an
    /// unexpected token.
    fn parse_type_cast(
        &mut self,
        expr: Expression<'ast>,
        level: usize,
        start: u32,
    ) -> Expression<'ast> {
        if !self.enter_wrapper(level) {
            return Expression::Error(expr.span());
        }
        self.advance();
        let ty = self.parse_type();
        self.nesting_depth = level;
        let span = Span::new(start, self.previous_end);
        Expression::new_type_cast(span, expr, ty, self.allocator)
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    #[inline]
    pub(super) fn parse_primary_expr(&mut self) -> Expression<'ast> {
        match self.peek() {
            name_token!() => {
                let start = self.token.span.start;
                let prefix = self.parse_name_expr();
                self.parse_suffixes(prefix, start)
            }
            TokenKind::Number => Expression::Number(self.advance_number()),
            TokenKind::StringLiteral => Expression::StringLiteral(self.advance_string()),
            TokenKind::Nil => Expression::Nil(self.advance()),
            TokenKind::False => Expression::False(self.advance()),
            TokenKind::True => Expression::True(self.advance()),
            TokenKind::LeftParen => self.parse_parenthesized(),
            TokenKind::LeftBrace => {
                let open = self.token.span;
                self.parse_table_constructor()
                    .map_or(Expression::Error(open), Expression::TableConstructor)
            }
            // `simpleexp ::= attributes 'function' funcbody`
            TokenKind::Function | TokenKind::At => self.parse_function_def(),
            TokenKind::DotDotDot => self.parse_vararg(),
            TokenKind::If => self.parse_if_expr(),
            TokenKind::InterpolatedStringBegin | TokenKind::InterpolatedStringSimple => {
                self.parse_interpolated_string()
            }
            _ => self.report_unexpected_in_expr(),
        }
    }

    /// The current name, whose kind the caller has matched, as an expression
    /// that reads it.
    #[inline]
    fn parse_name_expr(&mut self) -> Expression<'ast> {
        let name = self.advance_name();
        self.counts.name_uses += 1;
        Expression::Name(self.alloc(name))
    }

    fn parse_vararg(&mut self) -> Expression<'ast> {
        let span = self.advance();
        if !self.function_scope.is_vararg {
            self.error(span, "cannot use `...` outside a vararg function");
        }
        Expression::VarArg(span)
    }

    /// The tree keeps the parentheses around a call or `...`, which they
    /// truncate to one value. It records the rest and reads what they hold
    /// as if they weren't there, so they open no level.
    fn parse_parenthesized(&mut self) -> Expression<'ast> {
        let open = self.token.span;
        if !self.enter_grouping() {
            return Expression::Error(open);
        }
        let level = self.nesting_depth;
        let slot = self.groupings.open();
        self.advance();
        self.open(Closer::Paren);
        let expr = self.parse_expr(0);
        self.grouping_depth -= 1;
        let close = self.expect_closer(TokenKind::RightParen, Closer::Paren, open);
        let parens = open.merge(close);
        let grouped = if expr.is_multi_value() {
            if !self.enter_wrapper(level) {
                return Expression::Error(parens);
            }
            self.nesting_depth = level;
            Expression::new_parenthesized(parens, expr, self.allocator)
        } else {
            self.groupings.record(slot, expr.span(), parens);
            expr
        };
        self.parse_suffixes(grouped, open.start)
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

    /// The suffixes after `prefix`, whose first token is at `start`, loop
    /// into one node, so a run of any length is one level of nesting, counted
    /// toward the depth cap from the prefix's deepest level.
    ///
    /// Inlined apart from the run, so a prefix with no suffix makes no call,
    /// which measured 1% faster.
    #[inline]
    fn parse_suffixes(&mut self, prefix: Expression<'ast>, start: u32) -> Expression<'ast> {
        if self.is_at_suffix() { self.parse_suffix_run(prefix, start) } else { prefix }
    }

    /// A prefix that's a run already, which only parentheses can make, takes
    /// these suffixes into it, as `(a.b).c` is `a.b.c`.
    fn parse_suffix_run(&mut self, prefix: Expression<'ast>, start: u32) -> Expression<'ast> {
        let level = self.nesting_depth;
        if !matches!(prefix, Expression::Suffixed(_)) && !self.enter_wrapper(level) {
            return Expression::Error(prefix.span());
        }
        self.nesting_depth = level + 1;
        let suffixes_start = self.scratch.suffixes.len();
        loop {
            self.parse_suffix();
            if !self.is_at_suffix() {
                break;
            }
        }
        self.nesting_depth = level;
        let span = Span::new(start, self.previous_end);
        if let Expression::Suffixed(chain) = prefix {
            return self.join_suffixes(chain, suffixes_start, span);
        }
        // Most runs hold one suffix, which moves to the arena as a value.
        let suffixes = if self.scratch.suffixes.len() == suffixes_start + 1
            && let Some(suffix) = self.scratch.suffixes.pop()
        {
            OneOrMore::new(suffix, self.allocator)
        } else {
            OneOrMore::split_off_in(&mut self.scratch.suffixes, suffixes_start, self.allocator)
        };
        Expression::new_suffixed(span, prefix, suffixes, self.allocator)
    }

    /// Apart from the run, so its loop stays out of the frame that every
    /// level of nesting in a chain takes.
    #[cold]
    fn join_suffixes(
        &mut self,
        mut chain: Box<'ast, SuffixedExpression<'ast>>,
        suffixes_start: usize,
        span: Span,
    ) -> Expression<'ast> {
        let allocator = self.allocator;
        for suffix in self.scratch.suffixes.drain(suffixes_start..) {
            chain.suffixes.push(suffix, allocator);
        }
        chain.span = span;
        Expression::Suffixed(chain)
    }

    #[inline]
    fn is_at_suffix(&self) -> bool {
        let kind = self.peek();
        can_open_suffix(kind) && (kind != TokenKind::Less || self.is_at_type_instantiation())
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
                let close = self.expect_closer(TokenKind::RightBracket, Closer::Bracket, open);
                Suffix::Index { span: open.merge(close), index }
            }
            TokenKind::Colon => self.parse_method_call(),
            // Explicit type instantiation is a suffix, so it composes with
            // field access, indexing, and calls.
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
        self.is_at(TokenKind::Less) && self.peek_next() == TokenKind::Less
    }

    /// Parse `<<T, U>>`, returning the type arguments and the span of the
    /// final `>`.
    fn parse_type_instantiation(&mut self) -> (TypeArgs<'ast>, Span) {
        let open = self.advance(); // first `<`
        let type_args = self.parse_type_args(); // second `<` through first `>`
        let close = if self.is_at(TokenKind::Greater) {
            self.advance()
        } else {
            self.report_unclosed(TokenKind::Greater, open)
        };
        (type_args, close)
    }

    fn parse_if_expr(&mut self) -> Expression<'ast> {
        let if_span = self.token.span;
        if !self.enter_depth() {
            return Expression::Error(if_span);
        }
        self.advance();
        self.open(Closer::Else);
        let binding = self.parse_conditional_binding("if");
        // A binding holds its branch in a scope of its own, as a block does.
        self.counts.blocks += u32::from(binding.is_some());
        let condition = self.parse_expr(0);
        self.expect(TokenKind::Then);
        let then_expr = self.parse_expr(0);
        let start = self.scratch.elseif_exprs.len();
        while self.is_at(TokenKind::ElseIf) {
            let elseif_span = self.advance();
            let clause_binding = self.parse_conditional_binding("elseif");
            self.counts.blocks += u32::from(clause_binding.is_some());
            let clause_condition = self.parse_expr(0);
            self.expect(TokenKind::Then);
            let expr = self.parse_expr(0);
            let span = Span::new(elseif_span.start, self.previous_end);
            let clause = ElseIfExprClause::new(span, clause_binding, clause_condition, expr);
            self.scratch.elseif_exprs.push(clause);
        }
        self.close(Closer::Else);
        self.expect(TokenKind::Else);
        let else_expr = self.parse_expr(0);
        self.exit_depth();
        let span = Span::new(if_span.start, self.previous_end);
        let elseif_clauses =
            List::split_off_in(&mut self.scratch.elseif_exprs, start, self.allocator);
        Expression::new_if_expr(
            span,
            binding,
            condition,
            then_expr,
            elseif_clauses,
            else_expr,
            self.allocator,
        )
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
            let tail = self.interpolated_text(first);
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
        let mut open_hole = first;
        while self.is_at(TokenKind::InterpolatedStringMiddle) {
            open_hole = self.token;
            self.advance();
            self.parse_interpolated_hole(open_hole);
        }
        let (end_span, tail) = if self.is_at(TokenKind::InterpolatedStringEnd) {
            let end = self.token;
            self.advance();
            (end.span, self.interpolated_text(end))
        } else {
            // The `{` is the last byte of the text before the hole.
            let brace_start = open_hole.span.end - 1;
            let end_span = self
                .report_unclosed(TokenKind::RightBrace, Span::new(brace_start, open_hole.span.end));
            let at = self.previous_end;
            let missing = StringLiteral::new(ThinBytes::new(b""), None, Span::new(at, at));
            (end_span, missing)
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
        let text = self.interpolated_text(literal);
        self.scratch.segments.push(InterpolatedSegment::new(text, expr));
    }

    #[cold]
    #[inline(never)]
    fn report_empty_hole(&mut self, literal: Token) -> Expression<'ast> {
        let span = Span::new(literal.span.end - 1, self.token.span.start + 1);
        self.error(span, "malformed interpolated string: expected expression inside `{}`");
        Expression::Error(span)
    }

    /// The arguments sit one level under the run of suffixes, which
    /// [`Self::parse_suffix_run`] has already counted, so the parentheses add
    /// none of their own.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_function_args(&mut self) -> FunctionArgs<'ast> {
        match self.peek() {
            TokenKind::LeftParen => {
                // Luau rejects a call whose `(` starts on a new line ("ambiguous
                // syntax: function call x new statement").
                if self.token.has_line_break_before {
                    self.report_ambiguous_call();
                }
                let open = self.advance();
                self.open(Closer::Paren);
                let args = if self.is_at(TokenKind::RightParen) {
                    List::new()
                } else {
                    self.parse_expr_list()
                };
                let close = self.expect_closer(TokenKind::RightParen, Closer::Paren, open);
                FunctionArgs::Parenthesized { span: open.merge(close), args }
            }
            TokenKind::LeftBrace => {
                let open = self.token.span;
                self.parse_table_constructor()
                    .map_or(FunctionArgs::Error(open), FunctionArgs::TableConstructor)
            }
            TokenKind::StringLiteral => FunctionArgs::StringLiteral(self.advance_string()),
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
        let function_span = self.advance();
        let Some(body) = self.parse_function_body(function_span) else {
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

        // Luau rejects a leading separator (`{;}`, `{, 1}`). Consume it so
        // parsing recovers.
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

        let close = self.expect_closer(TokenKind::RightBrace, Closer::Brace, open);
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
            self.expect_closer(TokenKind::RightBracket, Closer::Bracket, open);
            self.expect(TokenKind::Equal);
            let value = self.parse_expr(0);
            let span = Span::new(open.start, self.previous_end);
            Field::Bracketed { span, key, value }
        } else if self.is_at_name() && self.peek_next() == TokenKind::Equal {
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
