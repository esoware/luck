use std::{fmt, mem};

use luck_allocator::{Box, List, ThinStr};
use luck_ast::{
    Block, ConditionalBinding, ElseClause, ElseIfClause, Expression, GetSpan as _, Statement,
    TypedName, Var,
};
use luck_token::{CompoundOp, Span, TokenKind};

use super::recovery::Closer;
use super::{Parser, name_token};

/// A compound statement, as its missing-keyword errors name it.
#[derive(Clone, Copy)]
enum Construct {
    IfStatement,
    WhileLoop,
    ForLoop,
}

impl fmt::Display for Construct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::IfStatement => "if-statement",
            Self::WhileLoop => "while-loop",
            Self::ForLoop => "for-loop",
        })
    }
}

/// Whether `kind` can open a statement, or is the `;` that may follow one.
pub(super) const fn is_stmt_start(kind: TokenKind) -> bool {
    match kind {
        name_token!(
            TokenKind::If
                | TokenKind::While
                | TokenKind::Do
                | TokenKind::For
                | TokenKind::Repeat
                | TokenKind::Function
                | TokenKind::Local
                | TokenKind::Semicolon
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::LeftParen
                | TokenKind::At
        ) => true,
        TokenKind::Number
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
        | TokenKind::Pipe
        | TokenKind::Dot
        | TokenKind::DotDot
        | TokenKind::DotDotDot
        | TokenKind::Colon
        | TokenKind::DoubleColon
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
/// index, or an assignment target, as Luau's parser reads it.
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
        self.block_depth += 1;
        self.counts.blocks += 1;

        let start = self.scratch.stmts.len();
        // There's no empty statement (`block ::= {stat [';']}`), so a `;` may
        // only follow a statement.
        let mut can_take_separator = false;
        loop {
            if self.is_at(TokenKind::Semicolon) {
                self.skip_semicolons(can_take_separator);
                can_take_separator = false;
            }

            if self.is_recovering() && !self.recover_at_statement() {
                continue;
            }

            // No closer begins a statement, so the block's end is checked for
            // last.
            let stmt = match self.peek() {
                TokenKind::Eof => break,
                kind if is_stmt_start(kind) => self.parse_stmt(),
                _ if self.is_at_expected_closer() => break,
                _ => self.report_unexpected_in_block(),
            };
            if stmt.has_to_end_block() {
                self.end_block_after(&stmt);
            }
            self.scratch.stmts.push(stmt);
            can_take_separator = true;
        }

        self.exit_depth();
        self.block_depth -= 1;

        let stmts = List::split_off_in(&mut self.scratch.stmts, start, self.allocator);
        let end_span = if stmts.is_empty() { start_span } else { self.previous_span };
        Some(Block::new(start_span.merge(end_span), stmts))
    }

    /// A run of `;`, which does nothing, each an error but a first that
    /// follows a statement.
    fn skip_semicolons(&mut self, can_take_separator: bool) {
        let mut can_take = can_take_separator;
        while self.is_at(TokenKind::Semicolon) {
            let span = self.advance();
            if !can_take {
                self.error(span, "unexpected token `;`");
            }
            can_take = false;
        }
    }

    /// A statement that has to end its block, a `return`, `break`, or
    /// `continue`, takes one `;`, which a `return` has taken already, and
    /// then the block ends. A statement after it is an error, and stays in
    /// the block, where it was written.
    fn end_block_after(&mut self, stmt: &Statement<'ast>) {
        let is_return = matches!(stmt, Statement::Return(_));
        if !is_return {
            self.eat(TokenKind::Semicolon);
        }
        if !self.is_at_block_end() && is_stmt_start(self.peek()) {
            let keyword = if is_return {
                "return"
            } else if matches!(stmt, Statement::Break(_)) {
                "break"
            } else {
                "continue"
            };
            let span = self.token.span;
            self.syntax_error(span, format!("`{keyword}` must be the last statement in its block"));
        }
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
    ///
    /// A contextual keyword starts its statement only when the next token
    /// continues one, so in `const = 1`, `const()`, and `const.x`, `const` is
    /// an ordinary name.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_stmt(&mut self) -> Statement<'ast> {
        let next = self.peek_next();
        match self.peek() {
            TokenKind::Local => self.parse_local_stmt(List::new()),
            TokenKind::If => self.parse_if_stmt(),
            TokenKind::Function => self.parse_function_decl(List::new()),
            TokenKind::For => self.parse_for_stmt(),
            TokenKind::While => self.parse_while_loop(),
            TokenKind::Do => self.parse_do_block(),
            TokenKind::Repeat => self.parse_repeat_loop(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Break => Statement::Break(self.advance_loop_exit("`break` outside a loop")),
            TokenKind::At => self.parse_attributed_function(),
            TokenKind::Continue if !continues_a_name(next) => {
                Statement::Continue(self.advance_loop_exit("`continue` outside a loop"))
            }
            TokenKind::Type if next.is_name() || next == TokenKind::Function => {
                self.parse_type_declaration(None)
            }
            TokenKind::Export
                if matches!(
                    next,
                    TokenKind::Local | TokenKind::Function | TokenKind::Type | TokenKind::Const
                ) =>
            {
                self.parse_export_stmt(List::new())
            }
            TokenKind::Const if next.is_name() || next == TokenKind::Function => {
                self.parse_local_stmt(List::new())
            }
            name_token!(TokenKind::LeftParen) => self.parse_assignment_or_call(),
            _ => self.report_unexpected_in_block(),
        }
    }

    fn parse_return_stmt(&mut self) -> Statement<'ast> {
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
        self.eat(TokenKind::Semicolon);
        let span = Span::new(return_span.start, self.previous_span.end);
        Statement::new_return(span, exprs, self.allocator)
    }

    /// The `local name =` or `const name =` that opens a condition after the
    /// `keyword` just read, if one does. A `const` before anything but a name
    /// is a variable named `const`, as in `if const then`.
    pub(super) fn parse_conditional_binding(
        &mut self,
        keyword: &str,
    ) -> Option<Box<'ast, ConditionalBinding<'ast>>> {
        let is_const = self.is_at(TokenKind::Const);
        let is_binding = self.is_at(TokenKind::Local) || (is_const && self.peek_next().is_name());
        if !is_binding {
            return None;
        }
        let binding_keyword = self.advance();
        let name = self.expect_identifier_recover();
        let typed_name = self.parse_typed_name(name);
        let span = binding_keyword.merge(self.previous_span);
        if self.is_at(TokenKind::Comma) {
            let local = if is_const { "const" } else { "local" };
            let comma = self.token.span;
            self.error(comma, format!("`{keyword} {local}` binds a single name"));
            while self.eat(TokenKind::Comma) {
                self.expect_identifier_recover();
                self.try_parse_type_annotation();
            }
        }
        self.expect(TokenKind::Equal);
        Some(ConditionalBinding::boxed(span, typed_name, is_const, self.allocator))
    }

    fn parse_if_stmt(&mut self) -> Statement<'ast> {
        let if_span = self.advance();
        self.open(Closer::BlockEnd);
        self.open(Closer::Else);
        let binding = self.parse_conditional_binding("if");
        let condition = self.parse_expr(0);
        self.expect_keyword(TokenKind::Then, Construct::IfStatement);
        let Some(block) = self.parse_block() else {
            return Statement::Error(if_span);
        };

        let start = self.scratch.elseif_clauses.len();
        while self.is_at(TokenKind::ElseIf) {
            let elseif_span = self.advance();
            let clause_binding = self.parse_conditional_binding("elseif");
            let clause_condition = self.parse_expr(0);
            self.expect_keyword(TokenKind::Then, Construct::IfStatement);
            let Some(clause_block) = self.parse_block() else {
                self.scratch.elseif_clauses.truncate(start);
                return Statement::Error(if_span);
            };
            let span = elseif_span.merge(clause_block.span);
            let clause = ElseIfClause::new(span, clause_binding, clause_condition, clause_block);
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
            Some(ElseClause::boxed(span, else_block, self.allocator))
        } else {
            None
        };

        self.close(Closer::Else);
        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, if_span);
        let span = if_span.merge(end_span);

        Statement::new_if_stmt(
            span,
            binding,
            condition,
            block,
            elseif_clauses,
            else_clause,
            self.allocator,
        )
    }

    fn parse_while_loop(&mut self) -> Statement<'ast> {
        let while_span = self.advance();
        self.open(Closer::BlockEnd);
        let binding = self.parse_conditional_binding("while");
        let condition = self.parse_expr(0);
        self.expect_keyword(TokenKind::Do, Construct::WhileLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(while_span);
        };
        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, while_span);
        let span = while_span.merge(end_span);
        Statement::new_while_loop(span, binding, condition, block, self.allocator)
    }

    fn parse_do_block(&mut self) -> Statement<'ast> {
        let do_span = self.advance();
        self.open(Closer::BlockEnd);
        let Some(block) = self.parse_block() else {
            return Statement::Error(do_span);
        };
        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, do_span);
        let span = do_span.merge(end_span);
        Statement::new_do_block(span, block, self.allocator)
    }

    fn parse_for_stmt(&mut self) -> Statement<'ast> {
        let for_span = self.advance();
        self.open(Closer::BlockEnd);
        let name = self.expect_identifier_recover();
        let first = self.parse_typed_name(name);

        // `name =` starts a numeric for; anything else is a generic for.
        if self.is_at(TokenKind::Equal) {
            self.parse_numeric_for(for_span, first)
        } else {
            self.parse_generic_for(for_span, first)
        }
    }

    fn parse_numeric_for(&mut self, for_span: Span, variable: TypedName<'ast>) -> Statement<'ast> {
        self.advance(); // `=`
        let start = self.parse_expr(0);
        self.expect_keyword(TokenKind::Comma, Construct::ForLoop);
        let limit = self.parse_expr(0);
        let step = self.eat(TokenKind::Comma).then(|| self.parse_expr(0));
        self.expect_keyword(TokenKind::Do, Construct::ForLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(for_span);
        };
        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, for_span);
        let span = for_span.merge(end_span);
        Statement::new_numeric_for(span, variable, start, limit, step, block, self.allocator)
    }

    fn parse_generic_for(&mut self, for_span: Span, first: TypedName<'ast>) -> Statement<'ast> {
        let start = self.scratch.typed_names.len();
        self.scratch.typed_names.push(first);
        while self.eat(TokenKind::Comma) {
            let name = self.expect_identifier_recover();
            let typed_name = self.parse_typed_name(name);
            self.scratch.typed_names.push(typed_name);
        }
        let names = List::split_off_in(&mut self.scratch.typed_names, start, self.allocator);

        self.expect_keyword(TokenKind::In, Construct::ForLoop);
        let exprs = self.parse_expr_list();
        self.expect_keyword(TokenKind::Do, Construct::ForLoop);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(for_span);
        };
        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, for_span);
        let span = for_span.merge(end_span);
        Statement::new_generic_for(span, names, exprs, block, self.allocator)
    }

    fn parse_repeat_loop(&mut self) -> Statement<'ast> {
        let repeat_span = self.advance();
        self.open(Closer::BlockEnd);
        let Some(block) = self.parse_loop_block() else {
            return Statement::Error(repeat_span);
        };
        self.expect_closer(TokenKind::Until, Closer::BlockEnd, repeat_span);
        let condition = self.parse_expr(0);
        let span = Span::new(repeat_span.start, self.previous_span.end);
        Statement::new_repeat_loop(span, block, condition, self.allocator)
    }

    fn parse_assignment_or_call(&mut self) -> Statement<'ast> {
        let start = self.token.span.start;
        let groupings_start = self.groupings.len();
        let expr = self.parse_expr(0);
        let grouping = self.groupings.around_since(groupings_start, expr.span());

        // Compound assignment: `var op= expr`.
        if let Some(op) = CompoundOp::from_token_kind(self.peek()) {
            self.advance();
            let target = self.ungrouped_target(expr, grouping);
            let value = self.parse_expr(0);
            let var = self.assignment_target(target);
            let span = Span::new(start, self.previous_span.end);
            return Statement::new_compound_assignment(span, var, op, value, self.allocator);
        }

        // Assignment: `varlist = explist`.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Equal) {
            let first = self.ungrouped_target(expr, grouping);
            return self.parse_assignment(first, start);
        }

        // Parentheses make a call a value, and the tree keeps the ones
        // around a call, so none here grouped one.
        if let Some(parens) = grouping {
            return self.report_expression_stmt(parens);
        }
        match expr {
            Expression::Suffixed(chain) if chain.is_call() => Statement::FunctionCall(chain),
            // Reported already.
            Expression::Error(span) => Statement::Error(span),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
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

    fn parse_assignment(&mut self, first: Expression<'ast>, start: u32) -> Statement<'ast> {
        let targets_start = self.scratch.exprs.len();
        self.scratch.exprs.push(first);
        while self.eat(TokenKind::Comma) {
            let groupings_start = self.groupings.len();
            let expr = self.parse_expr(0);
            let grouping = self.groupings.around_since(groupings_start, expr.span());
            let target = self.ungrouped_target(expr, grouping);
            self.scratch.exprs.push(target);
        }
        self.expect(TokenKind::Equal);
        let values = self.parse_expr_list();

        // Targets convert after the values parse, which decides how an
        // invalid target's error sorts among others at the same position.
        let mut exprs = mem::take(&mut self.scratch.exprs);
        let vars_start = self.scratch.vars.len();
        for target in exprs.drain(targets_start..) {
            let var = self.assignment_target(target);
            self.scratch.vars.push(var);
        }
        self.scratch.exprs = exprs;
        let targets = List::split_off_in(&mut self.scratch.vars, vars_start, self.allocator);
        let span = Span::new(start, self.previous_span.end);
        Statement::new_assignment(span, targets, values, self.allocator)
    }

    /// `target`, or an error where the source wrote it in the `grouping`
    /// parentheses, which make a variable a value, and so no target. The tree
    /// keeps no other trace of them.
    fn ungrouped_target(
        &mut self,
        target: Expression<'ast>,
        grouping: Option<Span>,
    ) -> Expression<'ast> {
        let Some(parens) = grouping else {
            return target;
        };
        self.error(parens, "invalid assignment target");
        Expression::Error(parens)
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
        self.syntax_error(span, format!("missing {kind} in {construct}"));
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
