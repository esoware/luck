//! Luau type grammar parser.
//!
//! Recursive descent over the type grammar, producing `luck_ast::types`
//! nodes. Precedence, loosest to tightest: union `|`, intersection `&`,
//! postfix `?`, primary. Function types (`(params) -> R`) are primaries
//! disambiguated from parenthesized types by the trailing `->`.

use luck_ast::shared::{ContainedSpan, Punctuated};
use luck_ast::types::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenType, TableType, Type, TypeArgs, TypeField,
    TypePack, TypeofType, UnionType, VariadicType,
};
use luck_token::{Span, Token, TokenKind};

use crate::parser::Parser;

/// Real Luau's flat type-suffix chain rejects `A | B & C`, `A? & B`, etc.
const MIXED_TYPE_MSG: &str =
    "mixing union and intersection types is not allowed; consider wrapping in parentheses";

impl Parser<'_> {
    /// Parse `: T` if present. All annotation sites share this so the
    /// Luau gate lives in exactly one place.
    pub fn try_parse_type_annotation(&mut self) -> Option<(Span, Type)> {
        if self.version.is_luau() && matches!(self.peek(), TokenKind::Colon) {
            let colon = self.advance_span();
            Some((colon, self.parse_type()))
        } else {
            None
        }
    }

    /// Parse a complete type. Entry point for every annotation position.
    pub fn parse_type(&mut self) -> Type {
        if let Err(err) = self.enter_depth() {
            self.errors.push(err);
            return Type::Error(self.current_span());
        }

        let result = match self.peek() {
            // Leading separator - multiline definition style:
            // `type T =` newline `| A` newline `| B`
            TokenKind::Pipe => {
                let leading_pipe = self.advance_span();
                let (types, last_span) = self.parse_separated_types(TokenKind::Pipe, true);
                Type::Union(Box::new(UnionType {
                    span: leading_pipe.merge(last_span),
                    leading_pipe: Some(leading_pipe),
                    types,
                }))
            }
            TokenKind::Ampersand => {
                let leading_ampersand = self.advance_span();
                let (types, last_span) = self.parse_separated_types(TokenKind::Ampersand, false);
                Type::Intersection(Box::new(IntersectionType {
                    span: leading_ampersand.merge(last_span),
                    leading_ampersand: Some(leading_ampersand),
                    types,
                }))
            }
            _ => self.parse_union_level(),
        };

        self.exit_depth();
        result
    }

    /// Collect `A sep B sep C` where every item was already known to be
    /// part of a separated list (used for the leading-separator forms).
    /// `across_intersections` controls whether items are intersection-level
    /// (union lists) or postfix-level (intersection lists).
    fn parse_separated_types(
        &mut self,
        separator: TokenKind,
        across_intersections: bool,
    ) -> (Punctuated<Type>, Span) {
        let check_mixing = |parser: &mut Self, item: &Type| {
            let is_mixed = if across_intersections {
                matches!(item, Type::Intersection(_))
            } else {
                matches!(item, Type::Optional(_))
            };
            if is_mixed {
                parser.error(item.span(), MIXED_TYPE_MSG.to_string());
            }
        };

        let mut pairs = Vec::new();
        let mut current = if across_intersections {
            self.parse_intersection_level()
        } else {
            self.parse_postfix_type()
        };
        check_mixing(self, &current);

        while std::mem::discriminant(self.peek()) == std::mem::discriminant(&separator) {
            let separator_span = self.advance_span();
            let next = if across_intersections {
                self.parse_intersection_level()
            } else {
                self.parse_postfix_type()
            };
            check_mixing(self, &next);
            pairs.push((current, separator_span));
            current = next;
        }

        let last_span = current.span();
        (Punctuated::from_pairs(pairs, Some(current)), last_span)
    }

    fn parse_union_level(&mut self) -> Type {
        let first = self.parse_intersection_level();
        if !matches!(self.peek(), TokenKind::Pipe) {
            return first;
        }

        // Real Luau's type suffix chain is flat: `A | B & C` (in either
        // order) is "Mixing union and intersection types is not allowed".
        if matches!(first, Type::Intersection(_)) {
            self.error(first.span(), MIXED_TYPE_MSG.to_string());
        }

        let start_span = first.span();
        let mut pairs = Vec::new();
        let mut current = first;
        while matches!(self.peek(), TokenKind::Pipe) {
            let pipe = self.advance_span();
            let next = self.parse_intersection_level();
            if matches!(next, Type::Intersection(_)) {
                self.error(next.span(), MIXED_TYPE_MSG.to_string());
            }
            pairs.push((current, pipe));
            current = next;
        }

        let span = start_span.merge(current.span());
        Type::Union(Box::new(UnionType {
            span,
            leading_pipe: None,
            types: Punctuated::from_pairs(pairs, Some(current)),
        }))
    }

    fn parse_intersection_level(&mut self) -> Type {
        let first = self.parse_postfix_type();
        if !matches!(self.peek(), TokenKind::Ampersand) {
            return first;
        }

        // `?` marks a union in real Luau, so `A? & B` and `A & B?` are
        // the same mixing error as `A | B & C`.
        if matches!(first, Type::Optional(_)) {
            self.error(first.span(), MIXED_TYPE_MSG.to_string());
        }

        let start_span = first.span();
        let mut pairs = Vec::new();
        let mut current = first;
        while matches!(self.peek(), TokenKind::Ampersand) {
            let ampersand = self.advance_span();
            let next = self.parse_postfix_type();
            if matches!(next, Type::Optional(_)) {
                self.error(next.span(), MIXED_TYPE_MSG.to_string());
            }
            pairs.push((current, ampersand));
            current = next;
        }

        let span = start_span.merge(current.span());
        Type::Intersection(Box::new(IntersectionType {
            span,
            leading_ampersand: None,
            types: Punctuated::from_pairs(pairs, Some(current)),
        }))
    }

    fn parse_postfix_type(&mut self) -> Type {
        let mut result = self.parse_primary_type();
        // `?` stacks (`T??`) - accepted permissively, same as Luau
        while matches!(self.peek(), TokenKind::Question) {
            let question = self.advance_span();
            let span = result.span().merge(question);
            result = Type::Optional(Box::new(OptionalType {
                span,
                type_value: result,
                question,
            }));
        }
        result
    }

    fn parse_primary_type(&mut self) -> Type {
        match self.peek() {
            TokenKind::Identifier(name)
                if name == "typeof" && matches!(self.peek_next(), TokenKind::LeftParen) =>
            {
                self.parse_typeof_type()
            }
            // `T...` - generic pack reference
            TokenKind::Identifier(_) if matches!(self.peek_next(), TokenKind::DotDotDot) => {
                let name = self.advance();
                let dots = self.advance_span();
                let span = name.span.merge(dots);
                Type::GenericPack(Box::new(GenericPackType { span, name, dots }))
            }
            TokenKind::Identifier(_) => {
                let name = self.advance();
                self.parse_named_type(name)
            }
            // Singletons. Number singletons are not valid Luau but were
            // historically accepted by the span scanner; stay permissive
            // so those sources keep round-tripping.
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::StringLiteral(_)
            | TokenKind::Number(_) => Type::Singleton(self.advance()),
            TokenKind::LeftBrace => self.parse_table_type(),
            TokenKind::LeftParen => self.parse_paren_or_function_type(),
            // `<T>(...) -> R` - generic function type
            TokenKind::Less => {
                let generics = self.parse_generic_type_list(false);
                self.parse_function_type(Some(generics))
            }
            // `...T` - variadic pack
            TokenKind::DotDotDot => {
                let dots = self.advance_span();
                let type_value = self.parse_type();
                let span = dots.merge(type_value.span());
                Type::Variadic(Box::new(VariadicType {
                    span,
                    dots,
                    type_value,
                }))
            }
            _ => {
                let span = self.current_span();
                self.error(span, format!("expected type, found {}", self.peek()));
                // Do not consume: the offending token may close an
                // enclosing construct or start the next statement.
                Type::Error(span)
            }
        }
    }

    fn parse_typeof_type(&mut self) -> Type {
        let typeof_token = self.advance_span();
        let open = self.advance_span(); // `(` - guaranteed by the caller's lookahead
        let expr = self.parse_expression(0);
        let close = self
            .expect_span(&TokenKind::RightParen)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        let span = typeof_token.merge(close);
        Type::Typeof(Box::new(TypeofType {
            span,
            typeof_token,
            parens: ContainedSpan { open, close },
            expr,
        }))
    }

    fn parse_named_type(&mut self, first_name: Token) -> Type {
        let (prefix, name) = if matches!(self.peek(), TokenKind::Dot) {
            let dot = self.advance_span();
            let name = self.expect_identifier().unwrap_or_else(|err| {
                self.errors.push(err);
                Token::new(
                    TokenKind::Identifier(String::new().into()),
                    self.current_span(),
                )
            });
            (Some((first_name, dot)), name)
        } else {
            (None, first_name)
        };

        let generics = if matches!(self.peek(), TokenKind::Less) {
            Some(self.parse_type_args())
        } else {
            None
        };

        let start_span = prefix
            .as_ref()
            .map(|(module, _)| module.span)
            .unwrap_or(name.span);
        let end_span = generics.as_ref().map(|args| args.span).unwrap_or(name.span);
        Type::Named(Box::new(NamedType {
            span: start_span.merge(end_span),
            prefix,
            name,
            generics,
        }))
    }

    /// Generic argument list at a use site: `<T, U..., (A, B)>`.
    fn parse_type_args(&mut self) -> TypeArgs {
        let open = self.advance_span(); // `<`
        let mut pairs = Vec::new();
        let mut current = None;

        while !matches!(
            self.peek(),
            TokenKind::Greater | TokenKind::ShiftRight | TokenKind::GreaterEqual | TokenKind::Eof
        ) {
            let arg = self.parse_type();
            let is_error = matches!(arg, Type::Error(_));
            if matches!(self.peek(), TokenKind::Comma) {
                let comma = self.advance_span();
                pairs.push((arg, comma));
            } else {
                current = Some(arg);
                break;
            }
            // An error without a following comma cannot make progress
            if is_error {
                break;
            }
        }

        let close = self.consume_type_close_angle();
        TypeArgs {
            span: open.merge(close),
            angles: ContainedSpan { open, close },
            args: Punctuated::from_pairs(pairs, current),
        }
    }

    /// Generic parameter list at a declaration site:
    /// `<T, U = string, V... = ...number>`.
    pub fn parse_generic_type_list(&mut self, allow_defaults: bool) -> GenericTypeList {
        let open = self.advance_span(); // `<`
        let mut pairs = Vec::new();
        let mut current = None;
        let mut seen_pack = false;
        let mut seen_default = false;

        while !matches!(
            self.peek(),
            TokenKind::Greater | TokenKind::ShiftRight | TokenKind::GreaterEqual | TokenKind::Eof
        ) {
            let name = match self.expect_identifier() {
                Ok(name) => name,
                Err(err) => {
                    self.errors.push(err);
                    break;
                }
            };
            let dots = if matches!(self.peek(), TokenKind::DotDotDot) {
                Some(self.advance_span())
            } else {
                None
            };
            let default = if matches!(self.peek(), TokenKind::Equal) {
                let equal = self.advance_span();
                Some((equal, self.parse_type()))
            } else {
                None
            };

            // Grammar: plain type parameters come before type packs, a
            // defaulted parameter forces defaults on the rest, pack
            // defaults must be packs, and defaults exist only in type
            // alias declarations.
            if dots.is_none() && seen_pack {
                self.error(
                    name.span,
                    "generic types come before generic type packs".to_string(),
                );
            }
            seen_pack |= dots.is_some();
            match &default {
                Some((_, default_type)) => {
                    if !allow_defaults {
                        self.error(
                            default_type.span(),
                            "default type parameters are only allowed in type alias declarations"
                                .to_string(),
                        );
                    }
                    let default_is_pack = matches!(
                        default_type,
                        Type::Pack(_) | Type::Variadic(_) | Type::GenericPack(_)
                    );
                    if dots.is_some() && !default_is_pack {
                        self.error(
                            default_type.span(),
                            "a generic type pack default must be a type pack".to_string(),
                        );
                    }
                    if dots.is_none() && default_is_pack {
                        self.error(
                            default_type.span(),
                            "a generic type default must be a single type".to_string(),
                        );
                    }
                    seen_default = true;
                }
                None => {
                    if dots.is_none() && seen_default {
                        self.error(
                            name.span,
                            "expected a default type after a defaulted type parameter".to_string(),
                        );
                    }
                }
            }

            let end_span = default
                .as_ref()
                .map(|(_, default_type)| default_type.span())
                .or(dots)
                .unwrap_or(name.span);
            let param = GenericTypeParam {
                span: name.span.merge(end_span),
                name,
                dots,
                default,
            };

            if matches!(self.peek(), TokenKind::Comma) {
                let comma = self.advance_span();
                pairs.push((param, comma));
            } else {
                current = Some(param);
                break;
            }
        }

        let close = self.consume_type_close_angle();
        GenericTypeList {
            span: open.merge(close),
            angles: ContainedSpan { open, close },
            params: Punctuated::from_pairs(pairs, current),
        }
    }

    fn parse_table_type(&mut self) -> Type {
        let open = self.advance_span(); // `{`
        let mut fields = Vec::new();

        while !matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
            let field = self.parse_type_field();
            let separator = if matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon) {
                Some(self.advance_span())
            } else {
                None
            };
            let is_last = separator.is_none();
            fields.push((field, separator));
            if is_last {
                break;
            }
        }

        // Grammar: `TableType ::= '{' Type '}' | '{' [PropList] '}'` -
        // an array table holds exactly one type, and a PropList holds at
        // most one indexer.
        let mut seen_indexer = false;
        for (idx, (field, _)) in fields.iter().enumerate() {
            match field {
                TypeField::Indexer { span, .. } => {
                    if seen_indexer {
                        self.error(*span, "cannot have more than one table indexer".to_string());
                    }
                    seen_indexer = true;
                }
                TypeField::Array { span, .. } => {
                    if idx > 0 || fields.len() > 1 {
                        self.error(
                            *span,
                            "an array-like table type holds exactly one element type; use named fields or an indexer instead".to_string(),
                        );
                        break;
                    }
                }
                TypeField::Named { .. } => {}
            }
        }

        let close = self
            .expect_span(&TokenKind::RightBrace)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        Type::Table(Box::new(TableType {
            span: open.merge(close),
            braces: ContainedSpan { open, close },
            fields,
        }))
    }

    fn parse_type_field(&mut self) -> TypeField {
        // `read`/`write` is a modifier only when a field follows it;
        // `{ read: number }` is a field literally named "read".
        let access = if let TokenKind::Identifier(name) = self.peek() {
            if (name == "read" || name == "write")
                && matches!(
                    self.peek_next(),
                    TokenKind::Identifier(_) | TokenKind::LeftBracket
                )
            {
                Some(self.advance())
            } else {
                None
            }
        } else {
            None
        };

        if matches!(self.peek(), TokenKind::LeftBracket) {
            let open = self.advance_span();
            let key = self.parse_type();
            let close = self
                .expect_span(&TokenKind::RightBracket)
                .unwrap_or_else(|err| {
                    self.errors.push(err);
                    self.current_span()
                });
            let colon = self.expect_span(&TokenKind::Colon).unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
            let value = self.parse_type();
            let start_span = access.as_ref().map(|token| token.span).unwrap_or(open);
            return TypeField::Indexer {
                span: start_span.merge(value.span()),
                access,
                brackets: ContainedSpan { open, close },
                key,
                colon,
                value,
            };
        }

        if self.check_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
            let name = self.advance();
            let colon = self.advance_span();
            let value = self.parse_type();
            let start_span = access.as_ref().map(|token| token.span).unwrap_or(name.span);
            return TypeField::Named {
                span: start_span.merge(value.span()),
                access,
                name,
                colon,
                value,
            };
        }

        if let Some(access_token) = access {
            self.error(
                access_token.span,
                "access modifier requires a named field or indexer".to_string(),
            );
        }
        let value = self.parse_type();
        TypeField::Array {
            span: value.span(),
            value,
        }
    }

    /// `(` opened: either a parenthesized type `(T)`, a pack `(T, U)`/`()`,
    /// or function-type params `(a: T, ...U) -> R` - decided by the `->`
    /// after the closing paren.
    fn parse_paren_or_function_type(&mut self) -> Type {
        let open = self.advance_span(); // `(`
        let mut pairs = Vec::new();
        let mut current = None;

        while !matches!(self.peek(), TokenKind::RightParen | TokenKind::Eof) {
            let param = if self.check_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
                let name = self.advance();
                let colon = self.advance_span();
                let type_value = self.parse_type();
                FunctionTypeParam {
                    span: name.span.merge(type_value.span()),
                    name: Some((name, colon)),
                    type_value,
                }
            } else {
                let type_value = self.parse_type();
                FunctionTypeParam {
                    span: type_value.span(),
                    name: None,
                    type_value,
                }
            };
            let is_error = matches!(param.type_value, Type::Error(_));

            if matches!(self.peek(), TokenKind::Comma) {
                let comma = self.advance_span();
                pairs.push((param, comma));
            } else {
                current = Some(param);
                break;
            }
            // An error without a following comma cannot make progress
            if is_error {
                break;
            }
        }

        let close = self
            .expect_span(&TokenKind::RightParen)
            .unwrap_or_else(|err| {
                self.errors.push(err);
                self.current_span()
            });
        let parens = ContainedSpan { open, close };
        let params = Punctuated::from_pairs(pairs, current);

        if matches!(self.peek(), TokenKind::Arrow) {
            let arrow = self.advance_span();
            let return_type = self.parse_type();
            let span = parens.open.merge(return_type.span());
            return Type::Function(Box::new(FunctionType {
                span,
                generics: None,
                parens,
                params,
                arrow,
                return_type,
            }));
        }

        // No arrow: plain parenthesized type or a pack. Param names are
        // only legal in function types.
        for param in params.iter() {
            if let Some((name, _)) = &param.name {
                self.error(
                    name.span,
                    "named parameters are only valid in function types".to_string(),
                );
            }
        }

        let span = parens.open.merge(parens.close);
        let mut type_items: Vec<(Type, Option<Span>)> = params
            .items
            .into_iter()
            .map(|(param, separator)| (param.type_value, separator))
            .collect();
        // `(T)` is a parenthesized type; `()`, `(T,)`, and `(T, U)` are packs
        if type_items.len() == 1 && type_items[0].1.is_none() {
            let (type_value, _) = type_items.remove(0);
            Type::Parenthesized(Box::new(ParenType {
                span,
                parens,
                type_value,
            }))
        } else {
            Type::Pack(Box::new(TypePack {
                span,
                parens,
                types: Punctuated { items: type_items },
            }))
        }
    }

    /// Function type whose generics were already consumed: `(params) -> R`.
    fn parse_function_type(&mut self, generics: Option<GenericTypeList>) -> Type {
        let start_span = generics
            .as_ref()
            .map(|list| list.span)
            .unwrap_or(self.current_span());

        if !matches!(self.peek(), TokenKind::LeftParen) {
            let span = self.current_span();
            self.error(
                span,
                format!("expected ( after generics, found {}", self.peek()),
            );
            return Type::Error(start_span.merge(span));
        }

        match self.parse_paren_or_function_type() {
            Type::Function(mut function_type) => {
                function_type.span = start_span.merge(function_type.span);
                function_type.generics = generics;
                Type::Function(function_type)
            }
            other => {
                // `<T>` followed by a paren type with no `->` - the generics
                // have nothing to attach to.
                self.error(
                    other.span(),
                    "expected -> after generic function type parameters".to_string(),
                );
                other
            }
        }
    }
}
