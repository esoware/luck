use core::mem;

use luck_ast::{
    Assignment, Block, CompoundAssignment, DoBlock, ElseClause, ElseIfClause, Expression,
    GenericFor, GotoStatement, IfStatement, LabelStatement, LastStatement, NumericFor, Parameter,
    Punctuated, RepeatLoop, ReturnStatement, Statement, Var, WhileLoop,
};
use luck_token::{CompoundOp, Span, TokenKind};

use super::Parser;
use super::skip::ExprTail;

/// A compound statement, as its missing-keyword errors name it.
#[derive(Clone, Copy)]
enum Construct {
    IfStatement,
    WhileLoop,
    DoBlock,
    ForLoop,
    RepeatLoop,
}

impl Construct {
    const fn display_name(self) -> &'static str {
        match self {
            Self::IfStatement => "if-statement",
            Self::WhileLoop => "while-loop",
            Self::DoBlock => "do-block",
            Self::ForLoop => "for-loop",
            Self::RepeatLoop => "repeat-loop",
        }
    }
}

/// Whether `kind` can open a statement in some dialect.
pub(super) const fn is_stmt_start(kind: &TokenKind) -> bool {
    match kind {
        TokenKind::If
        | TokenKind::While
        | TokenKind::Do
        | TokenKind::For
        | TokenKind::Repeat
        | TokenKind::Function
        | TokenKind::Local
        | TokenKind::Goto
        | TokenKind::DoubleColon
        | TokenKind::Semicolon
        | TokenKind::Return
        | TokenKind::Break
        | TokenKind::Identifier(_)
        | TokenKind::LeftParen
        | TokenKind::At => true,
        TokenKind::Number(_)
        | TokenKind::StringLiteral(_)
        | TokenKind::And
        | TokenKind::Else
        | TokenKind::ElseIf
        | TokenKind::End
        | TokenKind::False
        | TokenKind::In
        | TokenKind::Nil
        | TokenKind::Not
        | TokenKind::Or
        | TokenKind::Then
        | TokenKind::True
        | TokenKind::Until
        | TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::FloorDiv
        | TokenKind::Percent
        | TokenKind::Caret
        | TokenKind::Hash
        | TokenKind::Ampersand
        | TokenKind::Tilde
        | TokenKind::Pipe
        | TokenKind::ShiftLeft
        | TokenKind::ShiftRight
        | TokenKind::Dot
        | TokenKind::DotDot
        | TokenKind::DotDotDot
        | TokenKind::Colon
        | TokenKind::Comma
        | TokenKind::Equal
        | TokenKind::EqualEqual
        | TokenKind::TildeEqual
        | TokenKind::Less
        | TokenKind::LessEqual
        | TokenKind::Greater
        | TokenKind::GreaterEqual
        | TokenKind::RightParen
        | TokenKind::LeftBrace
        | TokenKind::RightBrace
        | TokenKind::LeftBracket
        | TokenKind::RightBracket
        | TokenKind::PlusEqual
        | TokenKind::MinusEqual
        | TokenKind::StarEqual
        | TokenKind::SlashEqual
        | TokenKind::FloorDivEqual
        | TokenKind::PercentEqual
        | TokenKind::CaretEqual
        | TokenKind::DotDotEqual
        | TokenKind::InterpolatedStringBegin(_)
        | TokenKind::InterpolatedStringMiddle(_)
        | TokenKind::InterpolatedStringEnd(_)
        | TokenKind::Arrow
        | TokenKind::Question
        | TokenKind::Eof => false,
    }
}

/// Whether `kind` can follow a last statement: its optional `;` or a token
/// that ends the block.
const fn can_follow_last_stmt(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Semicolon
            | TokenKind::End
            | TokenKind::Else
            | TokenKind::ElseIf
            | TokenKind::Until
            | TokenKind::Eof
    )
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// The caller has checked that the block fits.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_block(&mut self) -> Block {
        let start_span = self.current_span();
        self.enter_depth();
        self.block_depth = self.block_depth.saturating_add(1);

        let mut stmts = Vec::new();
        let mut last_stmt = None;
        // 5.1 and Luau have no empty statement (block ::= {stat [';']}), so
        // a `;` may only follow a statement. 5.2+ treats a lone `;` as a
        // statement.
        let mut can_take_separator = false;

        loop {
            while matches!(self.peek(), TokenKind::Semicolon) {
                if self.version.has_empty_stmt() {
                    let span = self.advance_span();
                    stmts.push(Statement::EmptyStatement(span));
                } else {
                    let span = self.current_span();
                    if !can_take_separator {
                        self.error(span, "unexpected token `;`");
                    }
                    self.advance_span();
                    can_take_separator = false;
                }
            }

            match self.peek() {
                TokenKind::Return => {
                    last_stmt = Some(Box::new(self.parse_return_stmt()));
                    break;
                }
                TokenKind::Break if self.version.has_last_stmt_break() => {
                    let span = self.advance_loop_exit("`break` outside a loop");
                    last_stmt = Some(Box::new(LastStatement::Break(span)));
                    self.eat(&TokenKind::Semicolon);
                    break;
                }
                // Block-ending tokens, left for the caller to consume.
                TokenKind::End
                | TokenKind::Else
                | TokenKind::ElseIf
                | TokenKind::Until
                | TokenKind::Eof => break,
                // Luau: `continue` is a contextual keyword, only ever a last
                // statement. Before anything else, as in `continue()` or
                // `continue = 5`, it's an ordinary name.
                TokenKind::Identifier(name)
                    if self.version.has_continue()
                        && name == "continue"
                        && can_follow_last_stmt(self.peek_next()) =>
                {
                    let span = self.advance_loop_exit("`continue` outside a loop");
                    last_stmt = Some(Box::new(LastStatement::Continue(span)));
                    self.eat(&TokenKind::Semicolon);
                    break;
                }
                kind if is_stmt_start(kind) => {
                    stmts.push(self.parse_stmt());
                    can_take_separator = true;
                }
                _ => {
                    stmts.push(self.report_unexpected_in_block());
                    can_take_separator = true;
                }
            }
        }

        self.exit_depth();
        self.block_depth = self.block_depth.saturating_sub(1);

        let end_span =
            if stmts.is_empty() && last_stmt.is_none() { start_span } else { self.previous_span };

        Block { span: start_span.merge(end_span), stmts, last_stmt }
    }

    #[cold]
    #[inline(never)]
    fn report_unexpected_in_block(&mut self) -> Statement {
        let span = self.current_span();
        self.error(span, format!("unexpected token {}", self.peek()));
        self.synchronize();
        Statement::Error(span)
    }

    fn parse_loop_block(&mut self) -> Block {
        let was_in_loop = mem::replace(&mut self.function_scope.is_in_loop, true);
        let block = self.parse_block();
        self.function_scope.is_in_loop = was_in_loop;
        block
    }

    /// Consume a `break` or `continue`, which is an error outside a loop.
    fn advance_loop_exit(&mut self, outside_loop_message: &'static str) -> Span {
        let span = self.advance_span();
        if !self.function_scope.is_in_loop {
            self.error(span, outside_loop_message);
        }
        span
    }

    /// Parse a single statement, or a [`Statement::Error`] after recovering
    /// from malformed input.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_stmt(&mut self) -> Statement {
        match self.peek() {
            TokenKind::If
            | TokenKind::While
            | TokenKind::Do
            | TokenKind::For
            | TokenKind::Repeat
                if !self.can_nest(1) =>
            {
                self.refuse_block_stmt()
            }
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::While => self.parse_while_loop(),
            TokenKind::Do => self.parse_do_block(),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::Repeat => self.parse_repeat_loop(),
            TokenKind::Function => self.parse_function_decl(Vec::new()),
            TokenKind::Local => self.parse_local_stmt(Vec::new()),
            // Lua 5.2+: `break` is an ordinary statement.
            TokenKind::Break if !self.version.has_last_stmt_break() => {
                Statement::Break(self.advance_loop_exit("`break` outside a loop"))
            }
            TokenKind::Goto if self.version.has_goto() => self.parse_goto_stmt(),
            TokenKind::DoubleColon if self.version.has_goto() => self.parse_label_stmt(),
            // Luau `@` attribute on function declarations.
            TokenKind::At if self.version.has_function_attributes() => {
                self.parse_attributed_function()
            }
            // Luau contextual keywords. `type`, `export`, and `const` start a
            // declaration only when the next token continues one; in
            // `const = 1`, `const()`, and `const.x`, `const` is an ordinary
            // identifier.
            TokenKind::Identifier(name)
                if self.version.has_type_syntax()
                    && name == "type"
                    && matches!(
                        self.peek_next(),
                        TokenKind::Identifier(_) | TokenKind::Function
                    ) =>
            {
                self.parse_type_declaration(None)
            }
            TokenKind::Identifier(name)
                if self.version.has_type_syntax()
                    && name == "export"
                    && (matches!(self.peek_next(), TokenKind::Local | TokenKind::Function)
                        || matches!(
                            self.peek_next(),
                            TokenKind::Identifier(next) if next == "type" || next == "const"
                        )) =>
            {
                self.parse_export_stmt(Vec::new())
            }
            TokenKind::Identifier(name)
                if self.version.has_const_stmt()
                    && name == "const"
                    && matches!(
                        self.peek_next(),
                        TokenKind::Identifier(_) | TokenKind::Function
                    ) =>
            {
                self.parse_const_stmt(Vec::new())
            }
            // Lua 5.5: `global` is an ordinary name unless the next token
            // continues a declaration.
            TokenKind::Identifier(name)
                if self.version.has_global()
                    && name == "global"
                    && matches!(
                        self.peek_next(),
                        TokenKind::Less
                            | TokenKind::Identifier(_)
                            | TokenKind::Star
                            | TokenKind::Function
                    ) =>
            {
                self.parse_global_stmt()
            }
            TokenKind::Identifier(_) | TokenKind::LeftParen => self.parse_assignment_or_call(),
            _ => self.report_unexpected_stmt_start(),
        }
    }

    /// Refuse the compound statement at the current keyword, whose block
    /// doesn't fit, skipping all of it.
    #[cold]
    #[inline(never)]
    fn refuse_block_stmt(&mut self) -> Statement {
        let start = self.current_span();
        self.report_depth_limit(start);
        let is_repeat = matches!(self.peek(), TokenKind::Repeat);
        self.skip_nested();
        if is_repeat {
            self.skip_expr(ExprTail::Operators(0));
        }
        Statement::Error(start.merge(self.previous_span))
    }

    #[cold]
    #[inline(never)]
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn report_unexpected_stmt_start(&mut self) -> Statement {
        let span = self.current_span();
        let kind = self.peek();
        let hint = match kind {
            TokenKind::End => " (extra `end` without matching block)",
            TokenKind::Else | TokenKind::ElseIf => " (`else`/`elseif` without matching `if`)",
            TokenKind::Until => " (`until` without matching `repeat`)",
            TokenKind::RightParen => " (unmatched `)`)",
            TokenKind::RightBrace => " (unmatched `}`)",
            _ => "",
        };
        self.error(span, format!("unexpected token {kind}{hint}"));
        self.synchronize();
        Statement::Error(span)
    }

    fn parse_return_stmt(&mut self) -> LastStatement {
        let return_span = self.advance_span();

        if self.function_scope.is_chunk {
            self.has_module_return = true;
            if self.has_value_exports {
                self.error(
                    return_span,
                    "a module cannot use both value exports and `return` statements",
                );
            }
        }

        let exprs = if can_follow_last_stmt(self.peek()) {
            Punctuated::empty()
        } else {
            self.parse_expr_list()
        };

        let semicolon = matches!(self.peek(), TokenKind::Semicolon).then(|| self.advance_span());

        let end_span =
            semicolon.or_else(|| exprs.last().map(Expression::span)).unwrap_or(return_span);

        LastStatement::Return(Box::new(ReturnStatement {
            span: return_span.merge(end_span),
            exprs,
        }))
    }

    fn parse_if_stmt(&mut self) -> Statement {
        let if_span = self.advance_span();
        let condition = self.parse_expr(0);
        self.expect_keyword(&TokenKind::Then, Construct::IfStatement);
        let block = self.parse_block();

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            elseif_clauses.push(self.parse_elseif_clause());
        }

        let else_clause = matches!(self.peek(), TokenKind::Else).then(|| self.parse_else_clause());

        let end_span = self.expect_keyword(&TokenKind::End, Construct::IfStatement);
        let span = if_span.merge(end_span);

        Statement::IfStatement(Box::new(IfStatement {
            span,
            condition,
            elseif_clauses,
            else_clause,
            block,
        }))
    }

    fn parse_elseif_clause(&mut self) -> ElseIfClause {
        let elseif_span = self.advance_span();
        let condition = self.parse_expr(0);
        self.expect_keyword(&TokenKind::Then, Construct::IfStatement);
        let block = self.parse_block();
        ElseIfClause { span: elseif_span.merge(block.span), condition, block }
    }

    fn parse_else_clause(&mut self) -> ElseClause {
        let else_span = self.advance_span();
        let block = self.parse_block();
        ElseClause { span: else_span.merge(block.span), block }
    }

    fn parse_while_loop(&mut self) -> Statement {
        let while_span = self.advance_span();
        let condition = self.parse_expr(0);
        self.expect_keyword(&TokenKind::Do, Construct::WhileLoop);
        let block = self.parse_loop_block();
        let end_span = self.expect_keyword(&TokenKind::End, Construct::WhileLoop);
        let span = while_span.merge(end_span);

        Statement::WhileLoop(Box::new(WhileLoop { span, condition, block }))
    }

    fn parse_do_block(&mut self) -> Statement {
        let do_span = self.advance_span();
        let block = self.parse_block();
        let end_span = self.expect_keyword(&TokenKind::End, Construct::DoBlock);
        let span = do_span.merge(end_span);

        Statement::DoBlock(Box::new(DoBlock { span, block }))
    }

    fn parse_for_stmt(&mut self) -> Statement {
        let for_span = self.advance_span();
        let first_name = self.expect_identifier_recover();
        let first = self.parse_parameter(first_name);

        // `name =` starts a numeric for; anything else is a generic for.
        if matches!(self.peek(), TokenKind::Equal) {
            self.parse_numeric_for(for_span, first)
        } else {
            self.parse_generic_for(for_span, first)
        }
    }

    fn parse_numeric_for(&mut self, for_span: Span, control: Parameter) -> Statement {
        let Parameter { name, type_annotation, .. } = control;
        self.advance_span(); // `=`
        let start = self.parse_expr(0);
        self.expect_keyword(&TokenKind::Comma, Construct::ForLoop);
        let limit = self.parse_expr(0);

        let step = self.eat(&TokenKind::Comma).then(|| self.parse_expr(0));

        self.expect_keyword(&TokenKind::Do, Construct::ForLoop);
        let block = self.parse_loop_block();
        let end_span = self.expect_keyword(&TokenKind::End, Construct::ForLoop);
        let span = for_span.merge(end_span);

        Statement::NumericFor(Box::new(NumericFor {
            span,
            name,
            type_annotation,
            start,
            limit,
            step,
            block,
        }))
    }

    fn parse_generic_for(&mut self, for_span: Span, first: Parameter) -> Statement {
        let mut params = vec![first];

        while self.eat(&TokenKind::Comma) {
            let name = self.expect_identifier_recover();
            params.push(self.parse_parameter(name));
        }

        let names = Punctuated::from_items(params);

        self.expect_keyword(&TokenKind::In, Construct::ForLoop);
        let exprs = self.parse_expr_list();
        self.expect_keyword(&TokenKind::Do, Construct::ForLoop);
        let block = self.parse_loop_block();
        let end_span = self.expect_keyword(&TokenKind::End, Construct::ForLoop);
        let span = for_span.merge(end_span);

        Statement::GenericFor(Box::new(GenericFor { span, names, exprs, block }))
    }

    fn parse_repeat_loop(&mut self) -> Statement {
        let repeat_span = self.advance_span();
        let block = self.parse_loop_block();
        self.expect_keyword(&TokenKind::Until, Construct::RepeatLoop);
        let condition = self.parse_expr(0);
        let span = repeat_span.merge(condition.span());

        Statement::RepeatLoop(Box::new(RepeatLoop { span, block, condition }))
    }

    fn parse_goto_stmt(&mut self) -> Statement {
        let goto_span = self.advance_span();
        let name = self.expect_identifier_recover();
        let span = goto_span.merge(name.span);
        Statement::Goto(Box::new(GotoStatement { span, name }))
    }

    fn parse_label_stmt(&mut self) -> Statement {
        let colons_open = self.advance_span();
        let name = self.expect_identifier_recover();
        let colons_close = self.expect(&TokenKind::DoubleColon);
        let span = colons_open.merge(colons_close);
        Statement::Label(Box::new(LabelStatement { span, name }))
    }

    fn parse_assignment_or_call(&mut self) -> Statement {
        let expr = self.parse_expr(0);

        // Luau compound assignment: `var op= expr`.
        if self.version.has_compound_assignment()
            && let Some(op) = CompoundOp::from_token_kind(self.peek())
        {
            return self.parse_compound_assignment(expr, op);
        }

        // Assignment: `varlist = explist`.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Equal) {
            return self.parse_assignment(expr);
        }

        let is_call = matches!(&expr, Expression::Suffixed(chain) if chain.is_call());
        if is_call && let Expression::Suffixed(call) = expr {
            Statement::FunctionCall(call)
        } else if let Expression::Error(span) = expr {
            // Reported already.
            Statement::Error(span)
        } else {
            self.report_expression_stmt(&expr)
        }
    }

    fn parse_compound_assignment(&mut self, target: Expression, op: CompoundOp) -> Statement {
        self.advance_span();
        let rhs = self.parse_expr(0);
        let var = self.assignment_target(target);
        let span = var.span().merge(rhs.span());
        Statement::CompoundAssignment(Box::new(CompoundAssignment { span, var, op, expr: rhs }))
    }

    fn parse_assignment(&mut self, first_target: Expression) -> Statement {
        let mut target_exprs = vec![first_target];
        while self.eat(&TokenKind::Comma) {
            target_exprs.push(self.parse_expr(0));
        }

        let equal = self.expect(&TokenKind::Equal);

        let values = self.parse_expr_list();

        // Targets convert after the values parse, which decides how an
        // invalid target's error sorts among others at the same position.
        let vars: Vec<Var> = target_exprs
            .into_iter()
            .map(|target_expr| self.assignment_target(target_expr))
            .collect();
        let targets = Punctuated::from_items(vars);
        let end_span = values.last().map_or(equal, Expression::span);
        let start_span = targets.first().map_or(equal, Var::span);
        let span = start_span.merge(end_span);

        Statement::Assignment(Box::new(Assignment { span, targets, values }))
    }

    #[cold]
    #[inline(never)]
    fn report_expression_stmt(&mut self, expr: &Expression) -> Statement {
        let span = expr.span();
        self.error(
            span,
            "expected function call or assignment (expressions are not valid statements)",
        );
        Statement::Error(span)
    }

    /// Unwrap an assignment target, or record an error and return a
    /// placeholder name when `expr` isn't a variable. An error expression has
    /// been reported already.
    fn assignment_target(&mut self, expr: Expression) -> Var {
        match expr {
            Expression::Name(token) => Var::Name(token),
            Expression::Suffixed(chain) if chain.is_var() => Var::Suffixed(chain),
            Expression::Error(span) => Var::Name(Self::recovery_identifier(span)),
            invalid @ (Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_)) => {
                let span = invalid.span();
                self.error(span, "invalid assignment target");
                Var::Name(Self::recovery_identifier(span))
            }
        }
    }

    /// Like [`Self::expect`], but the error names the construct missing the
    /// keyword.
    fn expect_keyword(&mut self, kind: &TokenKind, construct: Construct) -> Span {
        if self.is_at(kind) {
            return self.advance_span();
        }
        let span = self.current_span();
        self.error(span, format!("missing {kind} in {}", construct.display_name()));
        span
    }
}

#[cfg(test)]
mod tests {
    use luck_token::CompactString;

    use super::*;

    #[test]
    fn is_stmt_start_accepts_only_stmt_openers() {
        assert!(is_stmt_start(&TokenKind::If));
        assert!(is_stmt_start(&TokenKind::Local));
        assert!(is_stmt_start(&TokenKind::Identifier(CompactString::from("x"))));
        assert!(is_stmt_start(&TokenKind::LeftParen));
        assert!(is_stmt_start(&TokenKind::At));
        assert!(!is_stmt_start(&TokenKind::Plus));
        assert!(!is_stmt_start(&TokenKind::End));
        assert!(!is_stmt_start(&TokenKind::Eof));
    }
}
