//! Covers Lua `<const>`/`<close>` variable attributes (5.4+) and Luau
//! `@native`/`@[deprecated(...)]` function attributes.
//!
//! Both are validated as they're parsed, since Lua and Luau reject unknown attributes as syntax
//! errors.

use luck_allocator::List;
use luck_ast::{Attribute, Expression, Field, FunctionAttribute, GetSpan as _, Name};
use luck_token::{Span, TokenKind};

use super::{Closer, Parser};

impl<'ast> Parser<'ast> {
    /// Whether `expr` is a constant string, which a backtick string with no
    /// holes also is in Luau, and the parentheses recorded from
    /// `groupings_start` on don't hold it.
    fn is_constant_string(&self, expr: &Expression<'_>, groupings_start: usize) -> bool {
        let is_constant = match expr {
            Expression::StringLiteral(_) => true,
            // Luau
            Expression::InterpolatedString(interpolated) => interpolated.segments.is_empty(),
            Expression::Nil(_)
            | Expression::False(_)
            | Expression::True(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::TableConstructor(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_)
            | Expression::Error(_) => false,
        };
        is_constant && self.groupings.around_since(groupings_start, expr.span()).is_none()
    }

    /// Whether `expr` fits the grammar's `literal` production for attribute
    /// arguments: nil, booleans, numbers, strings, and tables of literals with
    /// plain-name or positional fields, none of them in the parentheses
    /// recorded from `groupings_start` on.
    ///
    /// An error passes, since it has been reported already.
    fn is_attribute_literal(&self, expr: &Expression<'_>, groupings_start: usize) -> bool {
        if self.groupings.around_since(groupings_start, expr.span()).is_some() {
            return false;
        }
        match expr {
            Expression::Nil(_)
            | Expression::True(_)
            | Expression::False(_)
            | Expression::Number(_)
            | Expression::Integer(_)
            | Expression::StringLiteral(_)
            | Expression::Error(_) => true,
            // Luau
            Expression::InterpolatedString(_) => self.is_constant_string(expr, groupings_start),
            Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
                Field::Named { value, .. } | Field::Positional(value) => {
                    self.is_attribute_literal(value, groupings_start)
                }
                Field::Bracketed { .. } => false,
            }),
            Expression::VarArg(_)
            | Expression::FunctionDef(_)
            | Expression::Name(_)
            | Expression::Suffixed(_)
            | Expression::Parenthesized(_)
            | Expression::BinaryOp(_)
            | Expression::UnaryOp(_)
            | Expression::IfExpression(_)
            | Expression::TypeCast(_) => false,
        }
    }

    pub(super) fn try_parse_attribute(&mut self) -> Option<Attribute<'ast>> {
        (self.is_at(TokenKind::Less) && self.version.has_attributes())
            .then(|| self.parse_attribute())
    }

    /// Parse a `< Name >` attribute; `<` must be the current token.
    pub(super) fn parse_attribute(&mut self) -> Attribute<'ast> {
        let open = self.advance(); // `<`
        let name = self.expect_identifier_recover();
        // 5.4 manual section 3.3.7: "There are two possible attributes"; Lua
        // rejects any other name at parse time.
        if !matches!(name.text.as_str(), "" | "const" | "close") {
            self.error(name.span, format!("unknown attribute `{}`", name.text));
        }
        let close = self.expect(TokenKind::Greater);
        Attribute::new(open.merge(close), name)
    }

    /// Grammar: `attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'`
    /// with `parattr ::= NAME [pars]` and literal-only arguments. The AST
    /// keeps them because `@native` and friends change how Luau compiles the
    /// function.
    pub(super) fn parse_function_attributes(&mut self) -> List<'ast, FunctionAttribute<'ast>> {
        let start = self.scratch.attributes.len();
        while self.is_at(TokenKind::At) {
            let at_span = self.advance();
            // Luau lexes `@name` and `@[` as single tokens.
            if self.token.span.start != at_span.end {
                self.error(at_span, "attribute name must directly follow `@`");
            }
            if self.eat(TokenKind::LeftBracket) {
                self.open(Closer::Bracket);
                loop {
                    let name = self.expect_identifier_recover();
                    let groupings_start = self.groupings.len();
                    let args = self.parse_attribute_args(groupings_start);
                    self.validate_function_attribute(name, args.as_deref(), start, groupings_start);
                    let span = Span::new(at_span.start, self.previous_span.end);
                    let attribute = FunctionAttribute::new(span, name, args);
                    self.scratch.attributes.push(attribute);
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.expect_closer(TokenKind::RightBracket, Closer::Bracket);
            } else {
                let name = self.expect_identifier_recover();
                self.validate_function_attribute(name, None, start, self.groupings.len());
                let attribute = FunctionAttribute::new(at_span.merge(name.span), name, None);
                self.scratch.attributes.push(attribute);
            }
        }
        List::split_off_in(&mut self.scratch.attributes, start, self.allocator)
    }

    /// Grammar: `pars ::= '(' [litlist] ')' | littable | STRING`. A string or
    /// table argument becomes a one-element argument list, so every form
    /// nests its arguments one level, as the list it prints back as does.
    ///
    /// Parentheses make a literal a value, so one the parentheses recorded
    /// from `groupings_start` on hold is no literal.
    #[expect(clippy::wildcard_enum_match_arm, reason = "the guard leaves only a string")]
    fn parse_attribute_args(
        &mut self,
        groupings_start: usize,
    ) -> Option<List<'ast, Expression<'ast>>> {
        if !matches!(
            self.peek(),
            TokenKind::LeftParen | TokenKind::StringLiteral | TokenKind::LeftBrace
        ) || !self.enter_depth()
        {
            return None;
        }
        let args = match self.peek() {
            TokenKind::LeftParen => {
                self.advance();
                self.open(Closer::Paren);
                let args = if self.is_at(TokenKind::RightParen) {
                    List::new()
                } else {
                    self.parse_expr_list()
                };
                self.expect_closer(TokenKind::RightParen, Closer::Paren);
                for expr in &args {
                    if !self.is_attribute_literal(expr, groupings_start) {
                        self.error(expr.span(), "attribute arguments must be literals");
                    }
                }
                args
            }
            TokenKind::LeftBrace => {
                let open = self.token.span;
                let expr = self
                    .parse_table_constructor()
                    .map_or(Expression::Error(open), Expression::TableConstructor);
                if !self.is_attribute_literal(&expr, groupings_start) {
                    self.error(expr.span(), "attribute arguments must be literals");
                }
                List::from_array_in([expr], self.allocator)
            }
            _ => {
                let literal = self.advance_string();
                List::from_array_in([Expression::StringLiteral(literal)], self.allocator)
            }
        };
        self.exit_depth();
        Some(args)
    }

    /// Luau validates attributes at parse time: only `checked`, `native`, and
    /// `deprecated` exist, duplicates are errors, and only `deprecated` takes
    /// arguments. The list's attributes so far are the scratch's from
    /// `list_start`.
    fn validate_function_attribute(
        &mut self,
        name: Name<'ast>,
        args: Option<&[Expression<'ast>]>,
        list_start: usize,
        groupings_start: usize,
    ) {
        if name.text.is_empty() {
            return;
        }
        if !matches!(name.text.as_str(), "checked" | "native" | "deprecated") {
            self.error(name.span, format!("invalid attribute `@{}`", name.text));
            return;
        }
        if args.is_some() && name.text != "deprecated" {
            self.error(name.span, format!("attribute `@{}` does not take arguments", name.text));
        }
        let is_duplicate = self
            .scratch
            .attributes
            .get(list_start..)
            .unwrap_or_default()
            .iter()
            .any(|attribute| attribute.name.text == name.text);
        if is_duplicate {
            self.error(name.span, format!("duplicate attribute `@{}`", name.text));
        }
        if name.text == "deprecated"
            && let Some(deprecated_args) = args
        {
            self.validate_deprecated_args(name.span, deprecated_args, groupings_start);
        }
    }

    /// Luau's `deprecatedArgsValidator`: at most one argument, a table whose
    /// only keys are `use` and `reason`, each holding a constant string.
    fn validate_deprecated_args(
        &mut self,
        name_span: Span,
        args: &[Expression<'ast>],
        groupings_start: usize,
    ) {
        let [arg] = args else {
            if !args.is_empty() {
                self.error(name_span, "`@deprecated` takes at most one argument");
            }
            return;
        };
        let is_grouped = self.groupings.around_since(groupings_start, arg.span()).is_some();
        let table = match arg {
            Expression::TableConstructor(table) if !is_grouped => table,
            Expression::Error(_) => return,
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
            | Expression::TypeCast(_) => {
                self.error(
                    arg.span(),
                    "`@deprecated` takes a table with `use` and `reason` fields",
                );
                return;
            }
        };
        for field in &table.fields {
            match field {
                Field::Named { name, value } => {
                    if !matches!(name.text.as_str(), "use" | "reason") {
                        self.error(
                            name.span,
                            "unknown `@deprecated` argument, only `use` and `reason` are allowed",
                        );
                        continue;
                    }
                    if !self.is_constant_string(value, groupings_start)
                        && !matches!(value, Expression::Error(_))
                    {
                        self.error(value.span(), "`@deprecated` fields take constant strings");
                    }
                }
                Field::Positional(_) | Field::Bracketed { .. } => {
                    self.error(field.span(), "`@deprecated` takes only `use` and `reason` fields");
                }
            }
        }
    }
}
