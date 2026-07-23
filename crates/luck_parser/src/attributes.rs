//! Attribute grammar for both dialects: Lua `<const>`/`<close>` variable
//! attributes (5.4+) and Luau `@native`/`@[deprecated(...)]` function
//! attributes. Both are parsed and validated at parse time, mirroring
//! real Lua and Luau, which reject unknown attributes as syntax errors.

use luck_ast::Expression;
use luck_ast::expr::Literal;
use luck_ast::shared::{Field, Punctuated};
use luck_ast::stmt::{Attribute, FunctionAttribute};
use luck_token::{Token, TokenKind};

use crate::parser::Parser;
use crate::stmt::punctuated_last_span;

/// Attribute arguments are restricted to the grammar's `literal`
/// production: nil, booleans, numbers, strings, and tables of literals
/// with plain-name or positional fields.
fn is_attribute_literal(expr: &Expression) -> bool {
    match expr {
        Expression::Nil(_)
        | Expression::True(_)
        | Expression::False(_)
        | Expression::Number(_)
        | Expression::Integer(_) // Luau
        | Expression::StringLiteral(_) => true,
        Expression::TableConstructor(table) => table.fields.iter().all(|field| match field {
            Field::Named { value, .. } | Field::Positional { value, .. } => {
                is_attribute_literal(value)
            }
            Field::Bracketed { .. } => false,
        }),
        _ => false,
    }
}

impl Parser<'_> {
    /// Try to parse `< Name >` attribute if the version supports it and `<` is next.
    pub(crate) fn try_parse_attribute(&mut self) -> Option<Attribute> {
        if self.version.has_attributes() && matches!(self.peek(), TokenKind::Less) {
            Some(self.parse_attribute())
        } else {
            None
        }
    }

    /// Parse `< Name >` attribute (assumes `<` is current token).
    pub(crate) fn parse_attribute(&mut self) -> Attribute {
        let open = self.advance_span(); // `<`
        let name = self.expect_identifier_recover();
        // 5.4 §3.3.7: "There are two possible attributes"; real Lua
        // rejects anything else at parse time.
        if let TokenKind::Identifier(attr_name) = &name.kind
            && !attr_name.is_empty()
            && attr_name != "const"
            && attr_name != "close"
        {
            self.error(name.span, format!("unknown attribute '{attr_name}'"));
        }
        let close = self.expect(&TokenKind::Greater);
        let span = open.merge(close);
        Attribute { span, name }
    }

    /// Parse `@native function ...` (Luau attributed function declaration).
    /// `@native` and friends change runtime codegen, so the attributes are
    /// kept on the AST and re-emitted - dropping them changes behavior.
    /// Covers both grammar forms:
    /// `attribute ::= '@' NAME | '@[' parattr {',' parattr} ']'` with
    /// `parattr ::= NAME [pars]` and literal-only arguments.
    pub(crate) fn parse_function_attributes(&mut self) -> Vec<FunctionAttribute> {
        let mut attributes = Vec::new();
        while matches!(self.peek(), TokenKind::At) {
            let at_token = self.advance_span(); // `@`
            if matches!(self.peek(), TokenKind::LeftBracket) {
                self.advance_span(); // `[`
                loop {
                    let name = self.expect_identifier_recover();
                    let args = self.parse_attribute_args();
                    self.validate_function_attribute(&name, args.is_some(), &attributes);
                    let end_span = args
                        .as_ref()
                        .and_then(punctuated_last_span)
                        .unwrap_or(name.span);
                    attributes.push(FunctionAttribute {
                        span: at_token.merge(end_span),
                        name,
                        args,
                    });
                    if matches!(self.peek(), TokenKind::Comma) {
                        self.advance_span();
                    } else {
                        break;
                    }
                }
                self.expect(&TokenKind::RightBracket);
            } else {
                let name = self.expect_identifier_recover();
                self.validate_function_attribute(&name, false, &attributes);
                attributes.push(FunctionAttribute {
                    span: at_token.merge(name.span),
                    name,
                    args: None,
                });
            }
        }
        attributes
    }

    /// Parse the optional `pars` of a parenthesized attribute:
    /// `pars ::= '(' [litlist] ')' | littable | STRING`. The string and
    /// table sugar canonicalize to a one-element argument list.
    fn parse_attribute_args(&mut self) -> Option<Punctuated<Expression>> {
        match self.peek() {
            TokenKind::LeftParen => {
                self.advance_span();
                let args = if matches!(self.peek(), TokenKind::RightParen) {
                    Punctuated::empty()
                } else {
                    self.parse_expression_list()
                };
                self.expect(&TokenKind::RightParen);
                for expr in args.iter() {
                    if !is_attribute_literal(expr) {
                        self.error(
                            expr.span(),
                            "attribute arguments must be literals".to_string(),
                        );
                    }
                }
                Some(args)
            }
            TokenKind::StringLiteral(_) => {
                let token = self.advance();
                let TokenKind::StringLiteral(text) = token.kind else {
                    unreachable!("peeked kind");
                };
                Some(Punctuated::from_item(Expression::StringLiteral(Literal {
                    text,
                    span: token.span,
                })))
            }
            TokenKind::LeftBrace => {
                let table = self.parse_table_constructor();
                let expr = Expression::TableConstructor(Box::new(table));
                if !is_attribute_literal(&expr) {
                    self.error(
                        expr.span(),
                        "attribute arguments must be literals".to_string(),
                    );
                }
                Some(Punctuated::from_item(expr))
            }
            _ => None,
        }
    }

    /// Luau validates attributes at parse time: only
    /// checked/native/deprecated exist, duplicates are errors, and only
    /// `deprecated` accepts arguments.
    fn validate_function_attribute(
        &mut self,
        name: &Token,
        has_args: bool,
        previous: &[FunctionAttribute],
    ) {
        let TokenKind::Identifier(attr_name) = &name.kind else {
            return;
        };
        if attr_name.is_empty() {
            return;
        }
        if !matches!(attr_name.as_str(), "checked" | "native" | "deprecated") {
            self.error(name.span, format!("invalid attribute '@{attr_name}'"));
            return;
        }
        if has_args && attr_name != "deprecated" {
            self.error(
                name.span,
                format!("attribute '@{attr_name}' does not take arguments"),
            );
        }
        if previous.iter().any(|prev| {
            matches!(&prev.name.kind, TokenKind::Identifier(existing) if existing == attr_name)
        }) {
            self.error(name.span, format!("duplicate attribute '@{attr_name}'"));
        }
    }
}
