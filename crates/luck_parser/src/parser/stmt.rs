use luck_ast::{
    Assignment, Attribute, AttributedName, Block, CompoundAssignment, DoBlock, ElseClause,
    ElseIfClause, Expression, FunctionAttribute, FunctionDecl, FunctionName, GenericFor,
    GlobalDeclaration, GlobalFunction, GlobalStar, GotoStatement, IfStatement, LabelStatement,
    LastStatement, LocalAssignment, LocalFunction, NumericFor, Parameter, Punctuated, RepeatLoop,
    ReturnStatement, Statement, TypeDeclaration, TypeDeclarationValue, Var, WhileLoop,
};
use luck_token::{CompoundOp, Span, Token, TokenKind};

use super::types::GenericsOwner;
use crate::parser::Parser;

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

/// The declaration that introduced a local binding, which fixes its
/// `is_const` and `is_exported` flags.
#[derive(Clone, Copy)]
enum LocalKind {
    Local,
    /// Luau `const`.
    Const,
    /// Luau `export local`.
    ExportedLocal,
    /// Luau `export const`, and `export function`, whose binding is read-only
    /// too.
    ExportedConst,
}

impl LocalKind {
    const fn is_const(self) -> bool {
        match self {
            Self::Const | Self::ExportedConst => true,
            Self::Local | Self::ExportedLocal => false,
        }
    }

    const fn is_exported(self) -> bool {
        match self {
            Self::ExportedLocal | Self::ExportedConst => true,
            Self::Local | Self::Const => false,
        }
    }
}

/// Whether `kind` can open a statement in some dialect.
pub(super) const fn is_statement_start(kind: &TokenKind) -> bool {
    match kind {
        TokenKind::If
        | TokenKind::While
        | TokenKind::Do
        | TokenKind::For
        | TokenKind::Repeat
        | TokenKind::Function
        | TokenKind::Local
        | TokenKind::Global
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
const fn can_follow_last_statement(kind: &TokenKind) -> bool {
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
    /// Parse a block: statements plus an optional trailing `return`, `break`,
    /// or `continue`.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_block(&mut self) -> Block {
        let start_span = self.current_span();
        if !self.enter_depth() {
            return Block { span: start_span, stmts: Vec::new(), last_stmt: None };
        }
        self.block_depth = self.block_depth.saturating_add(1);

        let mut stmts = Vec::new();
        let mut last_stmt = None;
        // 5.1 and Luau have no empty statement (block ::= {stat [';']}), so
        // a `;` may only follow a statement. 5.2+ treats a lone `;` as a
        // statement.
        let mut can_take_separator = false;

        loop {
            while matches!(self.peek(), TokenKind::Semicolon) {
                if self.version.has_empty_statement() {
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
                    last_stmt = Some(Box::new(self.parse_return_statement()));
                    break;
                }
                TokenKind::Break if self.version.has_last_statement_break() => {
                    let span = self.advance_span();
                    if self.loop_depth == 0 {
                        self.error(span, "`break` outside a loop");
                    }
                    last_stmt = Some(Box::new(LastStatement::Break(span)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance_span();
                    }
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
                        && can_follow_last_statement(self.peek_next()) =>
                {
                    let span = self.advance_span();
                    if self.loop_depth == 0 {
                        self.error(span, "`continue` outside a loop");
                    }
                    last_stmt = Some(Box::new(LastStatement::Continue(span)));
                    if matches!(self.peek(), TokenKind::Semicolon) {
                        self.advance_span();
                    }
                    break;
                }
                kind if is_statement_start(kind) => {
                    stmts.push(self.parse_statement());
                    can_take_separator = true;
                }
                _ => {
                    let span = self.current_span();
                    self.error(span, format!("unexpected token {}", self.peek()));
                    self.synchronize();
                    stmts.push(Statement::Error(span));
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

    /// Parse a loop body, where `break` and `continue` are valid.
    fn parse_loop_block(&mut self) -> Block {
        self.loop_depth = self.loop_depth.saturating_add(1);
        let block = self.parse_block();
        self.loop_depth = self.loop_depth.saturating_sub(1);
        block
    }

    /// Parse a single statement, or a [`Statement::Error`] after recovering
    /// from malformed input.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_statement(&mut self) -> Statement {
        match self.peek() {
            TokenKind::If => self.parse_if_statement(),
            TokenKind::While => self.parse_while_loop(),
            TokenKind::Do => self.parse_do_block(),
            TokenKind::For => self.parse_for_statement(),
            TokenKind::Repeat => self.parse_repeat_loop(),
            TokenKind::Function => self.parse_function_decl(Vec::new()),
            TokenKind::Local => self.parse_local_statement(Vec::new()),
            TokenKind::Break if !self.version.has_last_statement_break() => {
                // Lua 5.2+: `break` is an ordinary statement.
                let span = self.advance_span();
                if self.loop_depth == 0 {
                    self.error(span, "`break` outside a loop");
                }
                Statement::Break(span)
            }
            TokenKind::Goto if self.version.has_goto() => self.parse_goto_statement(),
            TokenKind::DoubleColon if self.version.has_goto() => self.parse_label_statement(),
            TokenKind::Global if self.version.has_global() => self.parse_global_statement(),
            // Luau `@` attribute on function declarations.
            TokenKind::At if self.version.is_luau() => self.parse_attributed_function(),
            // Luau contextual keywords. `type`, `export`, and `const` start a
            // declaration only when the next token continues one; in
            // `const = 1`, `const()`, and `const.x`, `const` is an ordinary
            // identifier.
            TokenKind::Identifier(name)
                if self.version.is_luau()
                    && name == "type"
                    && matches!(
                        self.peek_next(),
                        TokenKind::Identifier(_) | TokenKind::Function
                    ) =>
            {
                self.parse_type_declaration(None)
            }
            TokenKind::Identifier(name)
                if self.version.is_luau()
                    && name == "export"
                    && (matches!(self.peek_next(), TokenKind::Local | TokenKind::Function)
                        || matches!(
                            self.peek_next(),
                            TokenKind::Identifier(next) if next == "type" || next == "const"
                        )) =>
            {
                self.parse_export_statement(Vec::new())
            }
            TokenKind::Identifier(name)
                if self.version.is_luau()
                    && name == "const"
                    && matches!(
                        self.peek_next(),
                        TokenKind::Identifier(_) | TokenKind::Function
                    ) =>
            {
                self.parse_const_statement()
            }
            TokenKind::Identifier(_) | TokenKind::LeftParen => self.parse_assignment_or_call(),
            _ => {
                let span = self.current_span();
                let kind = self.peek();
                let hint = match kind {
                    TokenKind::End => " (extra `end` without matching block)",
                    TokenKind::Else | TokenKind::ElseIf => {
                        " (`else`/`elseif` without matching `if`)"
                    }
                    TokenKind::Until => " (`until` without matching `repeat`)",
                    TokenKind::RightParen => " (unmatched `)`)",
                    TokenKind::RightBrace => " (unmatched `}`)",
                    _ => "",
                };
                self.error(span, format!("unexpected token {kind}{hint}"));
                self.synchronize();
                Statement::Error(span)
            }
        }
    }

    fn parse_return_statement(&mut self) -> LastStatement {
        let return_span = self.advance_span(); // `return`

        if self.function_depth == 0 {
            self.has_module_return = true;
            if self.has_value_exports {
                self.error(
                    return_span,
                    "a module cannot use both value exports and `return` statements",
                );
            }
        }

        let exprs = if can_follow_last_statement(self.peek()) {
            Punctuated::empty()
        } else {
            self.parse_expression_list()
        };

        let semicolon = matches!(self.peek(), TokenKind::Semicolon).then(|| self.advance_span());

        let end_span =
            semicolon.or_else(|| exprs.last().map(Expression::span)).unwrap_or(return_span);

        LastStatement::Return(Box::new(ReturnStatement {
            span: return_span.merge(end_span),
            exprs,
        }))
    }

    fn parse_if_statement(&mut self) -> Statement {
        let if_span = self.advance_span();
        let condition = self.parse_expression(0);
        self.expect_keyword(&TokenKind::Then, Construct::IfStatement);
        let block = self.parse_block();

        let mut elseif_clauses = Vec::new();
        while matches!(self.peek(), TokenKind::ElseIf) {
            let elseif_span = self.advance_span();
            let elseif_condition = self.parse_expression(0);
            self.expect_keyword(&TokenKind::Then, Construct::IfStatement);
            let elseif_block = self.parse_block();
            let span = elseif_span.merge(elseif_block.span);
            elseif_clauses.push(ElseIfClause {
                span,
                condition: elseif_condition,
                block: elseif_block,
            });
        }

        let else_clause = matches!(self.peek(), TokenKind::Else).then(|| {
            let else_span = self.advance_span();
            let else_block = self.parse_block();
            let span = else_span.merge(else_block.span);
            ElseClause { span, block: else_block }
        });

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

    fn parse_while_loop(&mut self) -> Statement {
        let while_span = self.advance_span();
        let condition = self.parse_expression(0);
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

    fn parse_for_statement(&mut self) -> Statement {
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
        let start = self.parse_expression(0);
        self.expect_keyword(&TokenKind::Comma, Construct::ForLoop);
        let limit = self.parse_expression(0);

        let step = matches!(self.peek(), TokenKind::Comma).then(|| {
            self.advance_span();
            self.parse_expression(0)
        });

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

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            let name = self.expect_identifier_recover();
            params.push(self.parse_parameter(name));
        }

        let names = Punctuated::from_items(params);

        self.expect_keyword(&TokenKind::In, Construct::ForLoop);
        let exprs = self.parse_expression_list();
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
        let condition = self.parse_expression(0);
        let span = repeat_span.merge(condition.span());

        Statement::RepeatLoop(Box::new(RepeatLoop { span, block, condition }))
    }

    fn parse_function_decl(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let function_span = self.advance_span();
        let name = self.parse_func_name();
        let body = self.parse_function_body();
        let start_span = attributes.first().map_or(function_span, |attribute| attribute.span);
        let span = start_span.merge(body.span);

        Statement::FunctionDecl(Box::new(FunctionDecl { span, attributes, name, body }))
    }

    fn parse_func_name(&mut self) -> FunctionName {
        let first = self.expect_identifier_recover();
        let start_span = first.span;
        let mut names = vec![first];

        while matches!(self.peek(), TokenKind::Dot) {
            self.advance_span();
            let name = self.expect_identifier_recover();
            names.push(name);
        }

        let method = matches!(self.peek(), TokenKind::Colon).then(|| {
            self.advance_span();

            self.expect_identifier_recover()
        });

        let end_span = method
            .as_ref()
            .map(|name| name.span)
            .or_else(|| names.last().map(|name| name.span))
            .unwrap_or(start_span);

        FunctionName { span: start_span.merge(end_span), names, method }
    }

    fn parse_local_statement(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let local_span = self.advance_span();

        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(local_span, attributes, LocalKind::Local);
        }

        if let Some(attribute) = attributes.first() {
            self.error(attribute.span, "attributes are only allowed on function declarations");
        }
        self.parse_local_assignment(local_span, LocalKind::Local)
    }

    /// Parse a Luau `const` declaration: `const bindinglist '=' explist` or
    /// `const function NAME funcbody`.
    fn parse_const_statement(&mut self) -> Statement {
        let const_span = self.advance_span(); // contextual `const`
        if matches!(self.peek(), TokenKind::Function) {
            return self.parse_local_function(const_span, Vec::new(), LocalKind::Const);
        }
        self.parse_local_assignment(const_span, LocalKind::Const)
    }

    fn parse_local_function(
        &mut self,
        local_span: Span,
        attributes: Vec<FunctionAttribute>,
        kind: LocalKind,
    ) -> Statement {
        self.advance_span(); // `function`
        let name = self.expect_identifier_recover();
        let body = self.parse_function_body();
        let start_span = attributes.first().map_or(local_span, |attribute| attribute.span);
        let span = start_span.merge(body.span);

        Statement::LocalFunction(Box::new(LocalFunction {
            span,
            attributes,
            name,
            body,
            is_const: kind.is_const(),
            is_exported: kind.is_exported(),
        }))
    }

    fn parse_local_assignment(&mut self, local_span: Span, kind: LocalKind) -> Statement {
        let names = self.parse_attname_list();

        // 5.4 manual section 3.3.7: "A list of variables can contain at most
        // one to-be-closed variable."
        let mut has_close = false;
        for attribute in names.iter().filter_map(|attributed| attributed.attribute.as_deref()) {
            if attribute.name.identifier() == Some("close") {
                if has_close {
                    self.error(attribute.span, "multiple to-be-closed variables in local list");
                }
                has_close = true;
            }
        }

        let exprs = if matches!(self.peek(), TokenKind::Equal) {
            self.advance_span();
            Some(self.parse_expression_list())
        } else {
            // Grammar: `const bindinglist '=' explist`, so the initializer
            // isn't optional.
            if kind.is_const() {
                let span = self.current_span();
                self.error(span, "missing initializer in `const` declaration");
            }
            None
        };

        let end_span = exprs
            .as_ref()
            .and_then(Punctuated::last)
            .map(Expression::span)
            .or_else(|| names.last().map(|attributed| attributed.name.span))
            .unwrap_or(local_span);

        let span = local_span.merge(end_span);

        Statement::LocalAssignment(Box::new(LocalAssignment {
            span,
            names,
            exprs,
            is_const: kind.is_const(),
            is_exported: kind.is_exported(),
        }))
    }

    /// Parse `goto Name`.
    fn parse_goto_statement(&mut self) -> Statement {
        let goto_span = self.advance_span();
        let name = self.expect_identifier_recover();
        let span = goto_span.merge(name.span);
        Statement::Goto(Box::new(GotoStatement { span, name }))
    }

    /// Parse `:: Name ::`.
    fn parse_label_statement(&mut self) -> Statement {
        let colons_open = self.advance_span();
        let name = self.expect_identifier_recover();
        let colons_close = self.expect(&TokenKind::DoubleColon);
        let span = colons_open.merge(colons_close);
        Statement::Label(Box::new(LabelStatement { span, name }))
    }

    /// Parse a `global` statement: `global function`, `global [attrib] *`, or
    /// `global attnamelist`.
    fn parse_global_statement(&mut self) -> Statement {
        let global_span = self.advance_span();

        // `global function name(...) ... end`
        if matches!(self.peek(), TokenKind::Function) {
            self.advance_span(); // `function`
            let name = self.expect_identifier_recover();
            let body = self.parse_function_body();
            let span = global_span.merge(body.span);
            return Statement::GlobalFunction(Box::new(GlobalFunction { span, name, body }));
        }

        // `global *`
        if matches!(self.peek(), TokenKind::Star) {
            let star = self.advance_span();
            let span = global_span.merge(star);
            return Statement::GlobalStar(Box::new(GlobalStar { span, attribute: None }));
        }

        // `global <attrib> *` puts the attribute before the star. Otherwise
        // the attribute leads an attnamelist and the shared parser takes it.
        if matches!(self.peek(), TokenKind::Less) {
            let attribute = self.parse_attribute();
            if matches!(self.peek(), TokenKind::Star) {
                self.reject_global_close(&attribute);
                let star = self.advance_span();
                let span = global_span.merge(star);
                return Statement::GlobalStar(Box::new(GlobalStar {
                    span,
                    attribute: Some(attribute),
                }));
            }
            let names = self.parse_attributed_names(Some(&attribute));
            return self.finish_global_declaration(global_span, names);
        }

        // `global name [attrib] {, name [attrib]} ['=' explist]`
        let names = self.parse_attname_list();
        self.finish_global_declaration(global_span, names)
    }

    /// Parse the optional `= explist` tail of a `global` declaration
    /// (5.5 manual section 3.3.7: `stat ::= global attnamelist ['=' explist]`).
    fn finish_global_declaration(
        &mut self,
        global_span: Span,
        names: Punctuated<AttributedName>,
    ) -> Statement {
        for attribute in names.iter().filter_map(|attributed| attributed.attribute.as_deref()) {
            self.reject_global_close(attribute);
        }

        let exprs = matches!(self.peek(), TokenKind::Equal).then(|| {
            self.advance_span();
            self.parse_expression_list()
        });

        let end_span = exprs
            .as_ref()
            .and_then(Punctuated::last)
            .map(Expression::span)
            .or_else(|| names.last().map(|attributed| attributed.name.span))
            .unwrap_or(global_span);
        let span = global_span.merge(end_span);

        Statement::GlobalDeclaration(Box::new(GlobalDeclaration { span, names, exprs }))
    }

    /// 5.5 manual section 3.3.7: only local variables can have the close
    /// attribute.
    fn reject_global_close(&mut self, attribute: &Attribute) {
        if attribute.name.identifier() == Some("close") {
            self.error(attribute.span, "only local variables can have the `close` attribute");
        }
    }

    /// Parse an attnamelist: `[attrib] Name [attrib] {',' Name [attrib]}`.
    ///
    /// The leading attribute is new in 5.5; in 5.4, attributes only follow
    /// names.
    fn parse_attname_list(&mut self) -> Punctuated<AttributedName> {
        // Lua 5.5: optional leading attribute before the first name.
        let leading_attribute = (self.version.has_leading_attributes()
            && matches!(self.peek(), TokenKind::Less))
        .then(|| self.parse_attribute());
        self.parse_attributed_names(leading_attribute.as_ref())
    }

    /// Parse the names of an attnamelist after any leading attribute.
    /// `global <attrib> name...` also enters here directly.
    fn parse_attributed_names(
        &mut self,
        leading_attribute: Option<&Attribute>,
    ) -> Punctuated<AttributedName> {
        let Some(first) = self.parse_attributed_name(leading_attribute) else {
            return Punctuated::empty();
        };
        let mut names = vec![first];

        while matches!(self.peek(), TokenKind::Comma) {
            self.advance_span();
            let Some(name) = self.parse_attributed_name(leading_attribute) else {
                break;
            };
            names.push(name);
        }

        Punctuated::from_items(names)
    }

    /// Parse `Name [: T] [attrib]`, or `None` after recording the error when
    /// the name is missing.
    ///
    /// 5.5 manual section 3.3.7: a prefixed attribute applies to every name in
    /// the list, so each name gets a copy (emitting it after every name means
    /// the same thing). A name with its own trailing attribute too is an
    /// error, reported so that attribute isn't dropped silently.
    fn parse_attributed_name(
        &mut self,
        leading_attribute: Option<&Attribute>,
    ) -> Option<AttributedName> {
        let name = self.expect_identifier()?;
        // Luau: type annotation after the name.
        let type_annotation = self.try_parse_type_annotation();
        let trailing_attribute = self.try_parse_attribute();
        if let (Some(_), Some(redundant)) = (leading_attribute, &trailing_attribute) {
            self.error(redundant.span, "name already has a leading attribute");
        }
        let attribute = leading_attribute.cloned().or(trailing_attribute);
        Some(AttributedName { name, type_annotation, attribute: attribute.map(Box::new) })
    }

    fn parse_assignment_or_call(&mut self) -> Statement {
        let expr = self.parse_expression(0);

        // Luau compound assignment: `var op= expr`.
        if self.version.has_compound_assignment()
            && let Some(op) = CompoundOp::from_token_kind(self.peek())
        {
            self.advance_span();
            let rhs = self.parse_expression(0);
            let var = self.assignment_target(expr);
            let span = var.span().merge(rhs.span());
            return Statement::CompoundAssignment(Box::new(CompoundAssignment {
                span,
                var,
                op,
                expr: rhs,
            }));
        }

        // Assignment: `varlist = explist`.
        if matches!(self.peek(), TokenKind::Comma | TokenKind::Equal) {
            let mut target_exprs = vec![expr];
            while matches!(self.peek(), TokenKind::Comma) {
                self.advance_span();
                target_exprs.push(self.parse_expression(0));
            }

            let equal = self.expect(&TokenKind::Equal);

            let values = self.parse_expression_list();

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

            return Statement::Assignment(Box::new(Assignment { span, targets, values }));
        }

        if let Expression::FunctionCall(call) = expr {
            Statement::FunctionCall(call)
        } else {
            let span = expr.span();
            self.error(
                span,
                "expected function call or assignment (expressions are not valid statements)",
            );
            Statement::Error(span)
        }
    }

    /// Unwrap an assignment target, or record an error and return a
    /// placeholder name when `expr` isn't a variable.
    fn assignment_target(&mut self, expr: Expression) -> Var {
        if let Expression::Var(var) = expr {
            var
        } else {
            let span = expr.span();
            self.error(span, "invalid assignment target");
            Var::Name(Self::recovery_identifier(span))
        }
    }

    /// Parse a Luau type declaration: `type Name ['<' ... '>'] '=' Type` or
    /// `type function Name funcbody`.
    fn parse_type_declaration(&mut self, export_span: Option<Span>) -> Statement {
        let type_span = self.advance_span(); // `type` identifier
        let start_span = export_span.unwrap_or(type_span);

        // `type function Name funcbody` takes no `=`, and the body is
        // ordinary Luau.
        if matches!(self.peek(), TokenKind::Function) {
            self.advance_span(); // `function`
            let name = self.expect_identifier_recover();
            let body = self.parse_function_body();
            let span = start_span.merge(body.span);
            return Statement::TypeDeclaration(Box::new(TypeDeclaration {
                span,
                is_exported: export_span.is_some(),
                name,
                generics: None,
                type_value: TypeDeclarationValue::TypeFunction(Box::new(body)),
            }));
        }

        let name = self.expect_identifier_recover();

        let generics = matches!(self.peek(), TokenKind::Less)
            .then(|| Box::new(self.parse_generic_type_list(GenericsOwner::TypeAlias)));

        self.expect(&TokenKind::Equal);

        let alias_type = self.parse_type();
        let span = start_span.merge(alias_type.span());

        Statement::TypeDeclaration(Box::new(TypeDeclaration {
            span,
            is_exported: export_span.is_some(),
            name,
            generics,
            type_value: TypeDeclarationValue::Alias(alias_type),
        }))
    }

    /// Parse Luau `export type`, `export local`, `export const`, or
    /// `export function` after an optional function attribute list.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_export_statement(&mut self, attributes: Vec<FunctionAttribute>) -> Statement {
        let export_span = self.advance_span(); // contextual `export`

        if let TokenKind::Identifier(name) = self.peek()
            && name == "type"
        {
            if !attributes.is_empty() {
                self.error(export_span, "attributes are only allowed on exported functions");
            }
            return self.parse_type_declaration(Some(export_span));
        }

        if !self.version.has_value_exports() {
            self.error(export_span, "value exports are not supported in this Lua version");
            return Statement::Error(export_span);
        }

        if self.block_depth != 1 {
            self.error(export_span, "value exports are only allowed at module scope");
        }
        if self.has_module_return {
            self.error(
                export_span,
                "a module cannot use both value exports and `return` statements",
            );
        }
        self.has_value_exports = true;

        let statement = match self.peek() {
            TokenKind::Local => {
                let local_span = self.advance_span();
                if matches!(self.peek(), TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export local function` is not supported, use `export function`",
                    );
                    self.parse_local_function(local_span, attributes, LocalKind::Local)
                } else {
                    if !attributes.is_empty() {
                        self.error(
                            export_span,
                            "attributes are only allowed on exported functions",
                        );
                    }
                    self.parse_local_assignment(export_span, LocalKind::ExportedLocal)
                }
            }
            TokenKind::Function => {
                self.parse_local_function(export_span, attributes, LocalKind::ExportedConst)
            }
            TokenKind::Identifier(name) if name == "const" => {
                self.advance_span();
                if !attributes.is_empty() {
                    self.error(export_span, "attributes are only allowed on exported functions");
                }
                if matches!(self.peek(), TokenKind::Function) {
                    self.error(
                        export_span,
                        "`export const function` is not supported, use `export function`",
                    );
                    self.parse_local_function(export_span, Vec::new(), LocalKind::Const)
                } else {
                    self.parse_local_assignment(export_span, LocalKind::ExportedConst)
                }
            }
            _ => {
                let span = self.current_span();
                self.error(
                    span,
                    "`export` must be followed by `type`, `local`, `const`, or `function`",
                );
                return Statement::Error(export_span.merge(span));
            }
        };

        self.register_exported_names(&statement);
        statement
    }

    fn register_exported_names(&mut self, statement: &Statement) {
        if let Statement::LocalAssignment(local) = statement
            && local.is_exported
        {
            for attributed in local.names.iter() {
                self.register_exported_name(&attributed.name);
            }
        }
        if let Statement::LocalFunction(function) = statement
            && function.is_exported
        {
            self.register_exported_name(&function.name);
        }
    }

    fn register_exported_name(&mut self, token: &Token) {
        let TokenKind::Identifier(name) = &token.kind else {
            return;
        };
        if !self.exported_names.insert(name.clone()) {
            self.error(token.span, format!("duplicate exported identifier `{name}`"));
        }
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_attributed_function(&mut self) -> Statement {
        let attributes = self.parse_function_attributes();
        match self.peek() {
            TokenKind::Local => self.parse_local_statement(attributes),
            TokenKind::Function => self.parse_function_decl(attributes),
            TokenKind::Identifier(name) if name == "export" => {
                self.parse_export_statement(attributes)
            }
            _ => {
                let span = self.current_span();
                self.error(span, "expected `function`, `local`, or `export` after attribute");
                Statement::Error(span)
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
    fn is_statement_start_accepts_only_statement_openers() {
        assert!(is_statement_start(&TokenKind::If));
        assert!(is_statement_start(&TokenKind::Local));
        assert!(is_statement_start(&TokenKind::Identifier(CompactString::from("x"))));
        assert!(is_statement_start(&TokenKind::LeftParen));
        assert!(is_statement_start(&TokenKind::At));
        assert!(!is_statement_start(&TokenKind::Plus));
        assert!(!is_statement_start(&TokenKind::End));
        assert!(!is_statement_start(&TokenKind::Eof));
    }
}
