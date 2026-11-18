use core::mem;

use luck_allocator::{List, ThinStr};
use luck_ast::{
    Block, ElseClause, ElseIfClause, Expression, GetSpan as _, LastStatement, MAX_NESTING_DEPTH,
    Parameter, Statement, Var,
};
use luck_token::{CompoundOp, Span, TokenKind};

use super::{Closer, Parser};

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
pub(super) const fn is_stmt_start(kind: TokenKind) -> bool {
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
        | TokenKind::Identifier
        | TokenKind::LeftParen
        | TokenKind::At => true,
        TokenKind::Number
        | TokenKind::Integer
        | TokenKind::StringLiteral
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
        | TokenKind::InterpolatedStringBegin
        | TokenKind::InterpolatedStringMiddle
        | TokenKind::InterpolatedStringEnd
        | TokenKind::InterpolatedStringSimple
        | TokenKind::Arrow
        | TokenKind::Question
        | TokenKind::Eof => false,
    }
}

/// Whether `kind` can follow a last statement: its optional `;` or a token
/// that ends the block.
const fn can_follow_last_stmt(kind: TokenKind) -> bool {
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

/// Whether `kind` after `continue` makes it a name: a call, a field, an
/// index, or an assignment target, as Luau reads it.
const fn continues_a_name(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral
            | TokenKind::Dot
            | TokenKind::Colon
            | TokenKind::LeftBracket
            | TokenKind::Less
            | TokenKind::Equal
            | TokenKind::Comma
            | TokenKind::PlusEqual
            | TokenKind::MinusEqual
            | TokenKind::StarEqual
            | TokenKind::SlashEqual
            | TokenKind::FloorDivEqual
            | TokenKind::PercentEqual
            | TokenKind::CaretEqual
            | TokenKind::DotDotEqual
    )
}

impl<'ast> Parser<'ast> {
    /// The block runs to a closer something open expects, or to the end of
    /// the source, so a stray closer is an error inside it rather than the
    /// end of it. `None` when it would pass the depth cap.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_block(&mut self) -> Option<Block<'ast>> {
        let start_span = self.token.span;
        if !self.enter_depth() {
            return None;
        }
        self.block_depth = self.block_depth.wrapping_add(1);
        self.counts.blocks = self.counts.blocks.wrapping_add(1);

        let start = self.scratch.stmts.len();
        // 5.1 and Luau have no empty statement (block ::= {stat [';']}), so
        // a `;` may only follow a statement. 5.2+ treats a lone `;` as a
        // statement.
        let mut can_take_separator = false;

        let last_stmt = loop {
            if self.is_at(TokenKind::Semicolon) {
                self.parse_semicolons(can_take_separator);
                can_take_separator = false;
            }

            if self.is_recovering() && !self.recover_at_statement() {
                continue;
            }

            // No closer begins a statement, so the block's end is checked for
            // last. A last statement comes with the deepest level it reaches.
            let (last, deepest) = match self.peek() {
                TokenKind::Eof => break None,
                TokenKind::Return => {
                    let outer = self.begin_operand();
                    let last = self.parse_return_stmt();
                    let deepest = self.deepest_level;
                    self.end_operand(outer);
                    (last, deepest)
                }
                TokenKind::Break if self.version.has_last_stmt_break() => {
                    let span = self.advance_loop_exit("`break` outside a loop");
                    self.eat(TokenKind::Semicolon);
                    (LastStatement::Break(span), self.nesting_depth)
                }
                // Luau: `continue` is a contextual keyword, only ever a last
                // statement. Before what makes a name a call or a target, as
                // in `continue()` or `continue = 5`, it's an ordinary name.
                TokenKind::Identifier if self.is_at_continue() => {
                    let span = self.advance_loop_exit("`continue` outside a loop");
                    self.eat(TokenKind::Semicolon);
                    (LastStatement::Continue(span), self.nesting_depth)
                }
                kind if is_stmt_start(kind) => {
                    let stmt = self.parse_stmt();
                    self.scratch.stmts.push(stmt);
                    can_take_separator = true;
                    continue;
                }
                _ if self.is_at_expected_closer() => break None,
                _ => {
                    let stmt = self.report_unexpected_in_block();
                    self.scratch.stmts.push(stmt);
                    can_take_separator = true;
                    continue;
                }
            };
            if self.is_at_block_end() {
                break Some(last);
            }
            let stmt = self.misplaced_last_stmt(last, deepest);
            self.scratch.stmts.push(stmt);
            can_take_separator = true;
        };

        self.exit_depth();
        self.block_depth = self.block_depth.wrapping_sub(1);

        let stmts = List::split_off_in(&mut self.scratch.stmts, start, self.allocator);
        let end_span =
            if stmts.is_empty() && last_stmt.is_none() { start_span } else { self.previous_span };
        Some(Block::new(start_span.merge(end_span), stmts, last_stmt))
    }

    /// A run of `;`, each an empty statement in 5.2+, and in 5.1 and Luau an
    /// error unless it follows a statement.
    fn parse_semicolons(&mut self, can_take_separator: bool) {
        let mut can_take = can_take_separator;
        while self.is_at(TokenKind::Semicolon) {
            if self.version.has_empty_stmt() {
                let span = self.advance();
                self.scratch.stmts.push(Statement::EmptyStatement(span));
            } else {
                let span = self.advance();
                if !can_take {
                    self.error(span, "unexpected token `;`");
                }
                can_take = false;
            }
        }
    }

    fn is_at_continue(&self) -> bool {
        self.version.has_continue()
            && self.current_text() == "continue"
            && !continues_a_name(self.peek_next())
    }

    /// A token that begins no statement. The block goes on at the next place
    /// one reliably begins.
    #[cold]
    #[inline(never)]
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn report_unexpected_in_block(&mut self) -> Statement<'ast> {
        let span = self.token.span;
        let hint = match self.peek() {
            TokenKind::End => " (extra `end` without matching block)",
            TokenKind::Else | TokenKind::ElseIf => " (`else`/`elseif` without matching `if`)",
            TokenKind::Until => " (`until` without matching `repeat`)",
            TokenKind::RightParen => " (unmatched `)`)",
            TokenKind::RightBracket => " (unmatched `]`)",
            TokenKind::RightBrace => " (unmatched `}`)",
            _ => "",
        };
        let message = format!("unexpected token {}{hint}", self.token.display(self.source));
        self.syntax_error(span, message);
        self.advance();
        Statement::Error(span)
    }

    /// A `return`, or in 5.1 and Luau a `break` or `continue`, that a
    /// statement follows. It ends its block in every Lua, so the statements
    /// after it are an error, and it stands as the `do ... end` around it
    /// that would make them valid, so they and it stay in the tree. One whose
    /// `deepest` level is already the depth cap, which the wrapper would
    /// pass, is an error node instead.
    #[cold]
    #[inline(never)]
    fn misplaced_last_stmt(
        &mut self,
        last: LastStatement<'ast>,
        deepest: usize,
    ) -> Statement<'ast> {
        if is_stmt_start(self.peek()) {
            let keyword = match &last {
                LastStatement::Return(_) => "return",
                LastStatement::Break(_) => "break",
                LastStatement::Continue(_) => "continue",
                LastStatement::Error(_) => "",
            };
            let span = self.token.span;
            self.syntax_error(span, format!("`{keyword}` must be the last statement in its block"));
        }
        let span = last.span();
        if deepest >= MAX_NESTING_DEPTH {
            return Statement::Error(span);
        }
        let block = Block::new(span, List::new(), Some(last));
        Statement::new_do_block(span, block, self.allocator)
    }

    fn parse_loop_block(&mut self) -> Option<Block<'ast>> {
        let was_in_loop = mem::replace(&mut self.function_scope.is_in_loop, true);
        let block = self.parse_block();
        self.function_scope.is_in_loop = was_in_loop;
        block
    }

    /// Consume a `break` or `continue`, which is an error outside a loop.
    fn advance_loop_exit(&mut self, outside_loop_message: &'static str) -> Span {
        let span = self.advance();
        if !self.function_scope.is_in_loop {
            self.error(span, outside_loop_message);
        }
        span
    }

    /// Parse a single statement, or a [`Statement::Error`] after recovering
    /// from malformed input.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_stmt(&mut self) -> Statement<'ast> {
        match self.peek() {
            TokenKind::Identifier => self.parse_identifier_stmt(),
            TokenKind::Local => self.parse_local_stmt(List::new()),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::Function => self.parse_function_decl(List::new()),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::While => self.parse_while_loop(),
            TokenKind::Do => self.parse_do_block(),
            TokenKind::Repeat => self.parse_repeat_loop(),
            TokenKind::LeftParen => self.parse_assignment_or_call(),
            // Lua 5.2+: `break` is an ordinary statement.
            TokenKind::Break if !self.version.has_last_stmt_break() => {
                Statement::Break(self.advance_loop_exit("`break` outside a loop"))
            }
            // Lua 5.2+
            TokenKind::Goto if self.version.has_goto() => self.parse_goto_stmt(),
            // Lua 5.2+
            TokenKind::DoubleColon if self.version.has_goto() => self.parse_label_stmt(),
            // Luau `@` attribute on function declarations.
            TokenKind::At if self.version.has_function_attributes() => {
                self.parse_attributed_function()
            }
            _ => self.report_unexpected_in_block(),
        }
    }

    /// Luau's `type`, `export`, and `const`, and Lua 5.5's `global`, are
    /// contextual keywords: each starts a declaration only when the next
    /// token continues one, so in `const = 1`, `const()`, and `const.x`,
    /// `const` is an ordinary name.
    fn parse_identifier_stmt(&mut self) -> Statement<'ast> {
        let next = self.peek_next();
        match self.current_text() {
            // Luau
            "type"
                if self.version.has_type_syntax()
                    && matches!(next, TokenKind::Identifier | TokenKind::Function) =>
            {
                self.parse_type_declaration(None)
            }
            // Luau
            "export"
                if self.version.has_type_syntax()
                    && (matches!(next, TokenKind::Local | TokenKind::Function)
                        || (next == TokenKind::Identifier
                            && matches!(self.next_text(), "type" | "const"))) =>
            {
                self.parse_export_stmt(List::new())
            }
            // Luau
            "const"
                if self.version.has_const_stmt()
                    && matches!(next, TokenKind::Identifier | TokenKind::Function) =>
            {
                self.parse_const_stmt(List::new())
            }
            // Lua 5.5
            "global"
                if self.version.has_global()
                    && matches!(
                        next,
                        TokenKind::Less
                            | TokenKind::Identifier
                            | TokenKind::Star
                            | TokenKind::Function
                    ) =>
            {
                self.parse_global_stmt()
            }
            _ => self.parse_assignment_or_call(),
        }
    }

    fn parse_return_stmt(&mut self) -> LastStatement<'ast> {
        let return_span = self.advance();

        if self.function_scope.is_chunk {
            self.has_module_return = true;
            if self.has_value_exports {
                self.error(
                    return_span,
                    "a module cannot use both value exports and `return` statements",
                );
            }
        }

        let exprs = if can_follow_last_stmt(self.peek()) || self.is_at_expected_closer() {
            List::new()
        } else {
            self.parse_expr_list()
        };

        let semicolon = self.is_at(TokenKind::Semicolon).then(|| self.advance());

        let end_span =
            semicolon.or_else(|| exprs.last().map(Expression::span)).unwrap_or(return_span);

        let span = return_span.merge(end_span);
        LastStatement::new_return(span, exprs, self.allocator)
    }

    fn parse_if_stmt(&mut self) -> Statement<'ast> {
        let if_span = self.advance();
        self.open(Closer::BlockEnd);
        self.open(Closer::Else);
        let condition = self.parse_expr(0);
        self.expect_keyword(TokenKind::Then, Construct::IfStatement);
        let Some(block) = self.parse_block() else {
            return Statement::Error(if_span);
        };

        let start = self.scratch.elseif_clauses.len();
        while self.is_at(TokenKind::ElseIf) {
            let elseif_span = self.advance();
            let clause_condition = self.parse_expr(0);
            self.expect_keyword(TokenKind::Then, Construct::IfStatement);
            let Some(clause_block) = self.parse_block() else {
                self.scratch.elseif_clauses.truncate(start);
                return Statement::Error(if_span);
            };
            let span = elseif_span.merge(clause_block.span);
            let clause = ElseIfClause::new(span, clause_condition, clause_block);
            self.scratch.elseif_clauses.push(clause);
        }
        let elseif_clauses =
            List::split_off_in(&mut self.scratch.elseif_clauses, start, self.allocator);

        let else_clause = if self.is_at(TokenKind::Else) {
            let else_span = self.advance();
            let Some(else_block) = self.parse_block() else {
                return Statement::Error(if_span);
            };
            let span = else_span.merge(else_block.span);
            Some(ElseClause::new(span, else_block))
        } else {
            None
        };

        self.close(Closer::Else);
        self.close(Closer::BlockEnd);
        let end_span = self.expect_keyword(TokenKind::End, Construct::IfStatement);
        let span = if_span.merge(end_span);

        Statement::new_if_stmt(span, condition, block, elseif_clauses, else_clause, self.allocator)
    }

    fn parse_while_loop(&mut self) -> Statement<'ast> {
        let while_span = self.advance();
        self.open(Closer::BlockEnd);
        let condition = self.parse_expr(0);
        self.expect_keyword(TokenKind::Do, Construct::WhileLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(while_span);
        };
        self.close(Closer::BlockEnd);
        let end_span = self.expect_keyword(TokenKind::End, Construct::WhileLoop);
        let span = while_span.merge(end_span);
        Statement::new_while_loop(span, condition, block, self.allocator)
    }

    fn parse_do_block(&mut self) -> Statement<'ast> {
        let do_span = self.advance();
        self.open(Closer::BlockEnd);
        let Some(block) = self.parse_block() else {
            return Statement::Error(do_span);
        };
        self.close(Closer::BlockEnd);
        let end_span = self.expect_keyword(TokenKind::End, Construct::DoBlock);
        let span = do_span.merge(end_span);
        Statement::new_do_block(span, block, self.allocator)
    }

    fn parse_for_stmt(&mut self) -> Statement<'ast> {
        let for_span = self.advance();
        self.open(Closer::BlockEnd);
        let name = self.expect_identifier_recover();
        let first = self.parse_parameter(name);

        // `name =` starts a numeric for; anything else is a generic for.
        if self.is_at(TokenKind::Equal) {
            self.parse_numeric_for(for_span, first)
        } else {
            self.parse_generic_for(for_span, first)
        }
    }

    fn parse_numeric_for(&mut self, for_span: Span, control: Parameter<'ast>) -> Statement<'ast> {
        let Parameter { name, type_annotation } = control;
        self.advance(); // `=`
        let start = self.parse_expr(0);
        self.expect_keyword(TokenKind::Comma, Construct::ForLoop);
        let limit = self.parse_expr(0);
        let step = self.eat(TokenKind::Comma).then(|| self.parse_expr(0));
        self.expect_keyword(TokenKind::Do, Construct::ForLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(for_span);
        };
        self.close(Closer::BlockEnd);
        let end_span = self.expect_keyword(TokenKind::End, Construct::ForLoop);
        let span = for_span.merge(end_span);
        Statement::new_numeric_for(
            span,
            name,
            type_annotation,
            start,
            limit,
            step,
            block,
            self.allocator,
        )
    }

    fn parse_generic_for(&mut self, for_span: Span, first: Parameter<'ast>) -> Statement<'ast> {
        let start = self.scratch.params.len();
        self.scratch.params.push(first);
        while self.eat(TokenKind::Comma) {
            let name = self.expect_identifier_recover();
            let param = self.parse_parameter(name);
            self.scratch.params.push(param);
        }
        let names = List::split_off_in(&mut self.scratch.params, start, self.allocator);

        self.expect_keyword(TokenKind::In, Construct::ForLoop);
        let exprs = self.parse_expr_list();
        self.expect_keyword(TokenKind::Do, Construct::ForLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(for_span);
        };
        self.close(Closer::BlockEnd);
        let end_span = self.expect_keyword(TokenKind::End, Construct::ForLoop);
        let span = for_span.merge(end_span);
        Statement::new_generic_for(span, names, exprs, block, self.allocator)
    }

    fn parse_repeat_loop(&mut self) -> Statement<'ast> {
        let repeat_span = self.advance();
        self.open(Closer::BlockEnd);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(repeat_span);
        };
        self.close(Closer::BlockEnd);
        self.expect_keyword(TokenKind::Until, Construct::RepeatLoop);
        let condition = self.parse_expr(0);
        let span = repeat_span.merge(condition.span());
        Statement::new_repeat_loop(span, block, condition, self.allocator)
    }

    fn parse_goto_stmt(&mut self) -> Statement<'ast> {
        let goto_span = self.advance();
        let name = self.expect_identifier_recover();
        let span = goto_span.merge(name.span);
        Statement::new_goto(span, name, self.allocator)
    }

    fn parse_label_stmt(&mut self) -> Statement<'ast> {
        let colons_open = self.advance();
        let name = self.expect_identifier_recover();
        let colons_close = self.expect(TokenKind::DoubleColon);
        let span = colons_open.merge(colons_close);
        Statement::new_label(span, name, self.allocator)
    }

    fn parse_assignment_or_call(&mut self) -> Statement<'ast> {
        let expr = self.parse_expr(0);

        // Luau compound assignment: `var op= expr`.
        if let Some(op) = CompoundOp::from_token_kind(self.peek())
            && self.version.has_compound_assignment()
        {
            self.advance();
            let value = self.parse_expr(0);
            let var = self.assignment_target(expr);
            let span = var.span().merge(value.span());
            return Statement::new_compound_assignment(span, var, op, value, self.allocator);
        }

        // Assignment: `varlist = explist`.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Equal) {
            return self.parse_assignment(expr);
        }

        match expr {
            Expression::Suffixed(chain) if chain.is_call() => Statement::FunctionCall(chain),
            // Reported already.
            Expression::Error(span) => Statement::Error(span),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::InterpolatedString(_)
            | Expression::TypeCast(_) => self.report_expression_stmt(expr.span()),
        }
    }

    fn parse_assignment(&mut self, first: Expression<'ast>) -> Statement<'ast> {
        let start = self.scratch.exprs.len();
        self.scratch.exprs.push(first);
        while self.eat(TokenKind::Comma) {
            let target = self.parse_expr(0);
            self.scratch.exprs.push(target);
        }
        let equal = self.expect(TokenKind::Equal);
        let values = self.parse_expr_list();

        // Targets convert after the values parse, which decides how an
        // invalid target's error sorts among others at the same position.
        let mut exprs = mem::take(&mut self.scratch.exprs);
        let vars_start = self.scratch.vars.len();
        for target in exprs.drain(start..) {
            let var = self.assignment_target(target);
            self.scratch.vars.push(var);
        }
        self.scratch.exprs = exprs;
        let targets = List::split_off_in(&mut self.scratch.vars, vars_start, self.allocator);

        let end_span = values.last().map_or(equal, Expression::span);
        let start_span = targets.first().map_or(equal, Var::span);
        let span = start_span.merge(end_span);
        Statement::new_assignment(span, targets, values, self.allocator)
    }

    #[cold]
    #[inline(never)]
    fn report_expression_stmt(&mut self, span: Span) -> Statement<'ast> {
        self.syntax_error(
            span,
            "expected function call or assignment (expressions are not valid statements)",
        );
        Statement::Error(span)
    }

    /// Unwrap an assignment target, or record an error and return an empty
    /// name when `expr` isn't a variable. An error expression has been
    /// reported already.
    fn assignment_target(&mut self, expr: Expression<'ast>) -> Var<'ast> {
        match expr {
            Expression::Name(name) => Var::Name(name),
            Expression::Suffixed(chain) if chain.is_var() => Var::Suffixed(chain),
            Expression::Error(span) => Var::new_name(ThinStr::new(""), None, span, self.allocator),
            Expression::Nil(_)
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
            | Expression::TypeCast(_) => {
                let span = expr.span();
                self.error(span, "invalid assignment target");
                Var::new_name(ThinStr::new(""), None, span, self.allocator)
            }
        }
    }

    /// Like [`Self::expect`], but the error names the construct missing the
    /// keyword.
    #[inline]
    fn expect_keyword(&mut self, kind: TokenKind, construct: Construct) -> Span {
        if self.is_at(kind) {
            return self.advance();
        }
        self.report_missing_keyword(kind, construct)
    }

    #[cold]
    #[inline(never)]
    fn report_missing_keyword(&mut self, kind: TokenKind, construct: Construct) -> Span {
        let span = self.token.span;
        self.syntax_error(span, format!("missing {kind} in {}", construct.display_name()));
        span
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stmt_start_accepts_only_stmt_openers() {
        assert!(is_stmt_start(TokenKind::If));
        assert!(is_stmt_start(TokenKind::Local));
        assert!(is_stmt_start(TokenKind::Identifier));
        assert!(is_stmt_start(TokenKind::LeftParen));
        assert!(is_stmt_start(TokenKind::At));
        assert!(!is_stmt_start(TokenKind::Plus));
        assert!(!is_stmt_start(TokenKind::End));
        assert!(!is_stmt_start(TokenKind::Eof));
    }
}
