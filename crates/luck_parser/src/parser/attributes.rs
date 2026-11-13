//! Covers Lua `<const>`/`<close>` variable attributes (5.4+) and Luau
//! `@native`/`@[deprecated(...)]` function attributes.
//!
//! Both are validated as they're parsed, since Lua and Luau reject unknown attributes as syntax
//! errors.

use luck_ast::{Attribute, Expression, Field, FunctionAttribute, Punctuated};
use luck_token::{Span, Token, TokenKind};

use super::Parser;

/// Whether `expr` is a constant string, which a backtick string with no holes
/// also is in Luau.
fn is_constant_string(expr: &Expression) -> bool {
    matches!(expr, Expression::StringLiteral(_))
        || matches!(
            expr,
            Expression::InterpolatedString(interpolated)
                if interpolated.segments.iter().all(|segment| segment.expr.is_none())
        )
}

/// Whether `expr` fits the grammar's `literal` production for attribute
/// arguments: nil, booleans, numbers, strings, and tables of literals with
/// plain-name or positional fields.
fn is_attribute_literal(expr: &Expression) -> bool {
    match expr {
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Number(_)
        | Expression::Integer(_)
        | Expression::StringLiteral(_) => true,
        // Luau
        Expression::InterpolatedString(_) => is_constant_string(expr),
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_attribute_literal(value)
            }
            Field::Bracketed { .. } => false,
        }),
        Expression::VarArg(_)
        | Expression::FunctionDef(_)
        | Expression::Var(_)
        | Expression::FunctionCall(_)
        | Expression::Parenthesized(_)
        | Expression::BinaryOp(_)
        | Expression::UnaryOp(_)
        | Expression::IfExpression(_)
        | Expression::TypeCast(_)
        | Expression::TypeInstantiation(_)
        | Expression::Error(_) => false,
    }
}

impl Parser<'_> {
    pub(super) fn try_parse_attribute(&mut self) -> Option<Attribute> {
        (self.version.has_attributes() && matches!(self.peek(), TokenKind::Less))
            .then(|| self.parse_attribute())
    }

    /// Parse a `< Name >` attribute; `<` must be the current token.
    pub(super) fn parse_attribute(&mut self) -> Attribute {
        let open = self.advance_span(); // `<`
        let name = self.expect_identifier_recover();
        // 5.4 manual section 3.3.7: "There are two possible attributes"; Lua
        // rejects any other name at parse time.
        if let TokenKind::Identifier(attribute_name) = &name.kind
            && !attribute_name.is_empty()
            && attribute_name != "const"
            && attribute_name != "close"
        {
            self.error(name.span, format!("unknown attribute `{attribute_name}`"));
        }
        let close = self.expect(&TokenKind::Greater);
        let span = open.merge(close);
        Attribute { span, name }
    }

    /// Grammar: `attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'`
    /// with `parattr ::= NAME [pars]` and literal-only arguments. The AST
    /// keeps them because `@native` and friends change how Luau compiles the
    /// function.
    pub(super) fn parse_function_attributes(&mut self) -> Vec<FunctionAttribute> {
        let mut attributes = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            let at_span = self.advance_span();
            // Luau lexes `@name` and `@[` as single tokens.
            if self.current_span().start != at_span.end {
                self.error(at_span, "attribute name must directly follow `@`");
            }
            if self.eat(&TokenKind::LeftBracket) {
                loop {
                    let name = self.expect_identifier_recover();
                    let args = self.parse_attribute_args();
                    self.validate_function_attribute(&name, args.as_ref(), &attributes);
                    let end_span = args
                        .as_ref()
                        .and_then(Punctuated::last)
                        .map_or(name.span, Expression::span);
                    attributes.push(FunctionAttribute {
                        span: at_span.merge(end_span),
                        name,
                        args,
                    });
                    if self.eat(&TokenKind::Comma) {
                    } else {
                        break;
                    }
                }
                self.expect(&TokenKind::RightBracket);
            } else {
                let name = self.expect_identifier_recover();
                self.validate_function_attribute(&name, None, &attributes);
                attributes.push(FunctionAttribute {
                    span: at_span.merge(name.span),
                    name,
                    args: None,
                });
            }
        }
        attributes
    }

    /// Grammar: `pars ::= '(' [litlist] ')' | littable | STRING`. A string or
    /// table argument becomes a one-element argument list, so every form
    /// nests its arguments one level, as the list it prints back as does.
    fn parse_attribute_args(&mut self) -> Option<Punctuated<Expression>> {
        if !matches!(
            self.peek(),
            TokenKind::LeftParen | TokenKind::StringLiteral(_) | TokenKind::LeftBrace
        ) || !self.enter_depth()
        {
            return None;
        }
        let args = self.parse_attribute_arg_list();
        self.exit_depth();
        args
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_attribute_arg_list(&mut self) -> Option<Punctuated<Expression>> {
        match self.peek() {
            TokenKind::LeftParen => {
                self.advance_span();
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expr_list()
                };
                self.expect(&TokenKind::RightParen);
                for expr in args.iter() {
                    if !is_attribute_literal(expr) {
                        self.error(expr.span(), "attribute arguments must be literals");
                    }
                }
                Some(args)
            }
            TokenKind::StringLiteral(_) => {
                Some(Punctuated::from_item(Expression::StringLiteral(self.advance_literal())))
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                let expr = Expression::TableConstructor(Box::new(table));
                if !is_attribute_literal(&expr) {
                    self.error(expr.span(), "attribute arguments must be literals");
                }
                Some(Punctuated::from_item(expr))
            }
            _ => None,
        }
    }

    /// Luau validates attributes at parse time: only `checked`, `native`, and
    /// `deprecated` exist, duplicates are errors, and only `deprecated` takes
    /// arguments.
    fn validate_function_attribute(
        &mut self,
        name: &Token,
        args: Option<&Punctuated<Expression>>,
        previous_attributes: &[FunctionAttribute],
    ) {
        let TokenKind::Identifier(attribute_name) = &name.kind else {
            return;
        };
        if attribute_name.is_empty() {
            return;
        }
        if !matches!(attribute_name.as_str(), "checked" | "native" | "deprecated") {
            self.error(name.span, format!("invalid attribute `@{attribute_name}`"));
            return;
        }
        if args.is_some() && attribute_name != "deprecated" {
            self.error(name.span, format!("attribute `@{attribute_name}` does not take arguments"));
        }
        if previous_attributes.iter().any(|attribute| {
            matches!(
                &attribute.name.kind,
                TokenKind::Identifier(existing) if existing == attribute_name
            )
        }) {
            self.error(name.span, format!("duplicate attribute `@{attribute_name}`"));
        }
        if attribute_name == "deprecated"
            && let Some(deprecated_args) = args
        {
            self.validate_deprecated_args(name.span, deprecated_args);
        }
    }

    /// Luau's `deprecatedArgsValidator`: at most one argument, a table whose
    /// only keys are `use` and `reason`, each holding a constant string.
    fn validate_deprecated_args(&mut self, name_span: Span, args: &Punctuated<Expression>) {
        if args.len() > 1 {
            self.error(name_span, "`@deprecated` takes at most one argument");
            return;
        }
        let Some(arg) = args.first() else {
            return;
        };
        let Expression::TableConstructor(table) = arg else {
            self.error(arg.span(), "`@deprecated` takes a table with `use` and `reason` fields");
            return;
        };
        for field in table.fields.iter() {
            match field {
                Field::Named { name, value, .. } => {
                    if !matches!(name.identifier(), Some("use" | "reason")) {
                        self.error(
                            name.span,
                            "unknown `@deprecated` argument, only `use` and `reason` are allowed",
                        );
                        continue;
                    }
                    if !is_constant_string(value) {
                        self.error(value.span(), "`@deprecated` fields take constant strings");
                    }
                }
                Field::Positional { span, .. } | Field::Bracketed { span, .. } => {
                    self.error(*span, "`@deprecated` takes only `use` and `reason` fields");
                }
            }
        }
    }
}
