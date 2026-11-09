//! Luau type grammar parser.
//!
//! Recursive descent over the type grammar, producing `luck_ast::Type`
//! nodes. Union (`|` and `?`) and intersection (`&`) suffixes can't mix
//! without parentheses. A `->` after the closing paren is what makes
//! `(params) -> R` a function type, and its return type may be a pack.

use luck_ast::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, Punctuated, TableType, Type,
    TypeArgs, TypeField, TypePack, TypeofType, UnionType, VariadicType,
};
use luck_token::{Span, Token, TokenKind, decode_string};

use super::Parser;

/// Luau's flat type-suffix chain rejects `A | B & C`, `A? & B`, and the like.
const MIXED_TYPE_MESSAGE: &str =
    "mixing union and intersection types is not allowed, consider wrapping in parentheses";

/// What declares a generic parameter list, which decides whether its
/// parameters may take defaults.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GenericsOwner {
    /// `type Name<T = U> = ...`, the only owner whose parameters take
    /// defaults.
    TypeAlias,
    /// A function body or a function type.
    Function,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    /// Parse `: T` if present, on a binding that takes a single type.
    pub(super) fn try_parse_type_annotation(&mut self) -> Option<Type> {
        (self.version.has_type_syntax() && self.eat(&TokenKind::Colon)).then(|| self.parse_type())
    }

    /// Whether `...T` or `T...` starts here.
    const fn is_at_type_pack(&self) -> bool {
        matches!(self.peek(), TokenKind::DotDotDot) || self.is_at_generic_pack()
    }

    const fn is_at_generic_pack(&self) -> bool {
        self.is_at_identifier() && matches!(self.peek_next(), TokenKind::DotDotDot)
    }

    /// Parse the `...T` or `T...` that [`Self::is_at_type_pack`] found.
    fn parse_type_pack(&mut self) -> Type {
        if self.is_at_generic_pack() {
            return self.parse_generic_pack();
        }
        let dots = self.advance_span();
        let type_value = self.parse_type();
        let span = dots.merge(type_value.span());
        Type::Variadic(Box::new(VariadicType { span, type_value }))
    }

    fn parse_generic_pack(&mut self) -> Type {
        let name = self.advance();
        let dots = self.advance_span();
        let span = name.span.merge(dots);
        Type::GenericPack(Box::new(GenericPackType { span, name }))
    }

    /// Parse a vararg parameter's annotation: a type, or a generic pack `T...`.
    pub(super) fn parse_vararg_type(&mut self) -> Type {
        if self.is_at_generic_pack() { self.parse_generic_pack() } else { self.parse_type() }
    }

    /// Parse a function's return type, which may be a type pack.
    pub(super) fn parse_return_type(&mut self) -> Type {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !matches!(self.peek(), TokenKind::LeftParen) {
            return self.parse_type();
        }
        if !self.enter_depth() {
            return Type::Error(self.current_span());
        }
        let mut first = self.parse_paren_or_function_type(true);
        // `parseReturnType` folds a suffix over `(T)` and over the `T` of
        // `(T, ...U)`, so `(A, B)?` and `() -> (A, B) | C` leave it unconsumed.
        if matches!(first, Type::Parenthesized(_)) {
            first = self.parse_union_level(first);
        }
        if let Type::Pack(pack) = &mut first
            && let [element, Type::Variadic(_) | Type::GenericPack(_)] =
                pack.types.items.as_mut_slice()
        {
            *element = self.parse_union_level(element.take());
            pack.span = pack.span.merge(element.span());
        }
        self.exit_depth();
        first
    }

    /// Parse a generic argument or a generic pack's default, which may be a
    /// type pack.
    fn parse_type_or_pack(&mut self) -> Type {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !matches!(self.peek(), TokenKind::LeftParen) {
            return self.parse_type();
        }
        if !self.enter_depth() {
            return Type::Error(self.current_span());
        }
        let first = self.parse_paren_or_function_type(true);
        let result =
            if matches!(first, Type::Pack(_)) { first } else { self.parse_union_level(first) };
        self.exit_depth();
        result
    }

    /// Parse a complete single type. A pack is valid only where
    /// [`Self::parse_return_type`] or [`Self::parse_type_or_pack`] takes one.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_type(&mut self) -> Type {
        if !self.enter_depth() {
            return Type::Error(self.current_span());
        }

        let result = match self.peek() {
            // A leading separator, as in the multiline definition style:
            // `type T =` newline `| A` newline `| B`.
            TokenKind::Pipe => {
                let leading_pipe = self.advance_span();
                let primary = self.parse_primary_type();
                let first = self.parse_intersection_level(primary);
                let types = self.parse_union_members(first);
                let last_span = types.last().map_or(leading_pipe, Type::span);
                Type::Union(Box::new(UnionType {
                    span: leading_pipe.merge(last_span),
                    has_leading_pipe: true,
                    types,
                }))
            }
            TokenKind::Ampersand => {
                let leading_ampersand = self.advance_span();
                let primary = self.parse_primary_type();
                let first = self.parse_postfix_type(primary);
                let types = self.parse_intersection_members(first);
                let last_span = types.last().map_or(leading_ampersand, Type::span);
                Type::Intersection(Box::new(IntersectionType {
                    span: leading_ampersand.merge(last_span),
                    has_leading_ampersand: true,
                    types,
                }))
            }
            _ => {
                let primary = self.parse_primary_type();
                self.parse_union_level(primary)
            }
        };

        self.exit_depth();
        result
    }

    /// Parse the suffixes of the union whose first primary is `primary`.
    fn parse_union_level(&mut self, primary: Type) -> Type {
        let first = self.parse_intersection_level(primary);
        if !matches!(self.peek(), TokenKind::Pipe) {
            return first;
        }
        let start_span = first.span();
        let types = self.parse_union_members(first);
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Union(Box::new(UnionType { span, has_leading_pipe: false, types }))
    }

    /// Parse the `| B | C` after a union's `first` member. Each member is an
    /// intersection-level type.
    ///
    /// Luau's type suffix chain is flat, so `A & B | C` and `A | B & C` are
    /// both mixing errors, not a precedence question.
    fn parse_union_members(&mut self, first: Type) -> Punctuated<Type> {
        if matches!(first, Type::Intersection(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let mut members = vec![first];
        while self.eat(&TokenKind::Pipe) {
            let primary = self.parse_primary_type();
            let next = self.parse_intersection_level(primary);
            if matches!(next, Type::Intersection(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            members.push(next);
        }
        Punctuated::from_items(members)
    }

    fn parse_intersection_level(&mut self, primary: Type) -> Type {
        let first = self.parse_postfix_type(primary);
        if !matches!(self.peek(), TokenKind::Ampersand) {
            return first;
        }
        let start_span = first.span();
        let types = self.parse_intersection_members(first);
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Intersection(Box::new(IntersectionType { span, has_leading_ampersand: false, types }))
    }

    /// Parse the `& B & C` after an intersection's `first` member. Each
    /// member is a postfix-level type.
    ///
    /// `?` is a union in Luau, so `A? & B` and `A & B?` are the same mixing
    /// error as `A | B & C`.
    fn parse_intersection_members(&mut self, first: Type) -> Punctuated<Type> {
        if matches!(first, Type::Optional(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let mut members = vec![first];
        while self.eat(&TokenKind::Ampersand) {
            let primary = self.parse_primary_type();
            let next = self.parse_postfix_type(primary);
            if matches!(next, Type::Optional(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            members.push(next);
        }
        Punctuated::from_items(members)
    }

    fn parse_postfix_type(&mut self, primary: Type) -> Type {
        let mut result = primary;
        // `?` stacks (`T??`), which Luau also accepts.
        while matches!(self.peek(), TokenKind::Question) {
            let question = self.advance_span();
            let span = result.span().merge(question);
            result = Type::Optional(Box::new(OptionalType { span, type_value: result }));
        }
        result
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_primary_type(&mut self) -> Type {
        match self.peek() {
            // `typeof.Name` is a module-qualified type.
            TokenKind::Identifier(name)
                if name == "typeof" && !matches!(self.peek_next(), TokenKind::Dot) =>
            {
                self.parse_typeof_type()
            }
            TokenKind::Identifier(_) => {
                let name = self.advance();
                self.parse_named_type(name)
            }
            // Singletons. Luau has no number singleton, but accepting one
            // keeps sources that carry it round-tripping.
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::StringLiteral(_)
            | TokenKind::Number(_) => Type::Singleton(self.advance()),
            TokenKind::LeftBrace => self.parse_table_type(),
            TokenKind::LeftParen => self.parse_paren_or_function_type(false),
            // `<T>(...) -> R` is a generic function type.
            TokenKind::Less => {
                let generics = self.parse_generic_type_list(GenericsOwner::Function);
                self.parse_function_type(generics)
            }
            _ => {
                let span = self.current_span();
                self.error(span, format!("expected type, found {}", self.peek()));
                // Don't consume the token; it may close an enclosing construct
                // or start the next statement.
                Type::Error(span)
            }
        }
    }

    fn parse_typeof_type(&mut self) -> Type {
        let typeof_span = self.advance_span();
        if !matches!(self.peek(), TokenKind::LeftParen) {
            self.report_expected(&TokenKind::LeftParen);
            return Type::Error(typeof_span);
        }
        self.advance_span();
        let expr = self.parse_expr(0);
        let close = self.expect(&TokenKind::RightParen);
        let span = typeof_span.merge(close);
        Type::Typeof(Box::new(TypeofType { span, expr }))
    }

    fn parse_named_type(&mut self, first_name: Token) -> Type {
        if !matches!(self.peek(), TokenKind::Dot | TokenKind::Less) {
            #[expect(
                clippy::unreachable,
                reason = "the caller has already matched this token kind"
            )]
            let TokenKind::Identifier(name) = first_name.kind else {
                unreachable!("caller matched an identifier");
            };
            return Type::Name { name, span: first_name.span };
        }
        let (prefix, name) = if self.eat(&TokenKind::Dot) {
            let name = self.expect_identifier_recover();
            (Some(first_name), name)
        } else {
            (None, first_name)
        };

        let generics = matches!(self.peek(), TokenKind::Less).then(|| self.parse_type_args());

        let start_span = prefix.as_ref().map_or(name.span, |module| module.span);
        let end_span = generics.as_ref().map_or(name.span, |args| args.span);
        Type::Named(Box::new(NamedType {
            span: start_span.merge(end_span),
            prefix,
            name,
            generics,
        }))
    }

    /// Generic argument list at a use site: `<T, U..., (A, B)>`.
    pub(super) fn parse_type_args(&mut self) -> TypeArgs {
        let open = self.advance_span(); // `<`
        let mut args = Punctuated::<Type>::empty();

        if !matches!(
            self.peek(),
            TokenKind::Greater | TokenKind::ShiftRight | TokenKind::GreaterEqual | TokenKind::Eof
        ) {
            loop {
                let arg = self.parse_type_or_pack();
                let is_error = matches!(arg, Type::Error(_));
                args.push(arg);
                if !self.eat(&TokenKind::Comma) || is_error {
                    break;
                }
            }
        }

        let close = self.consume_type_close_angle();
        TypeArgs { span: open.merge(close), args }
    }

    /// Generic parameter list at a declaration site:
    /// `<T, U = string, V... = ...number>`.
    pub(super) fn parse_generic_type_list(&mut self, owner: GenericsOwner) -> GenericTypeList {
        let open = self.advance_span(); // `<`
        let mut params = Punctuated::<GenericTypeParam>::empty();
        let mut has_seen_pack = false;
        let mut has_seen_default = false;

        loop {
            let Some(name) = self.expect_identifier() else {
                break;
            };
            let dots = matches!(self.peek(), TokenKind::DotDotDot).then(|| self.advance_span());
            let default = self.eat(&TokenKind::Equal).then(|| {
                if dots.is_some() { self.parse_type_or_pack() } else { self.parse_type() }
            });

            // Grammar: plain parameters come before packs, a parameter after
            // a defaulted one needs a default too, a pack's default must be a
            // pack, and only type alias declarations take defaults.
            if dots.is_none() && has_seen_pack {
                self.error(name.span, "generic types come before generic type packs");
            }
            has_seen_pack |= dots.is_some();
            match &default {
                Some(default_type) => {
                    if owner != GenericsOwner::TypeAlias {
                        self.error(
                            default_type.span(),
                            "default type parameters are only allowed in type alias declarations",
                        );
                    }
                    // `(T)` in a pack position is a one-type pack.
                    let is_pack_default = matches!(
                        default_type,
                        Type::Pack(_)
                            | Type::Variadic(_)
                            | Type::GenericPack(_)
                            | Type::Parenthesized(_)
                    );
                    if dots.is_some() && !is_pack_default {
                        self.error(
                            default_type.span(),
                            "a generic type pack default must be a type pack",
                        );
                    }
                    has_seen_default = true;
                }
                None => {
                    if has_seen_default {
                        self.error(
                            name.span,
                            "expected a default type after a defaulted type parameter",
                        );
                    }
                }
            }

            let end_span = default.as_ref().map(Type::span).or(dots).unwrap_or(name.span);
            let param = GenericTypeParam {
                span: name.span.merge(end_span),
                name,
                is_pack: dots.is_some(),
                default,
            };
            params.push(param);

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let close = self.consume_type_close_angle();
        GenericTypeList { span: open.merge(close), params }
    }

    /// Consume the `>` that closes a generic list, recording an error if it's
    /// missing.
    ///
    /// The lexer is greedy, so `Foo<Bar<T>>` ends in `ShiftRight` and
    /// `Foo<T>=x` holds `GreaterEqual`. Both get split here: this returns the
    /// first `>`'s span and leaves the rest as the current token.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn consume_type_close_angle(&mut self) -> Span {
        match self.peek() {
            TokenKind::Greater => self.advance_span(),
            TokenKind::ShiftRight => self.split_close_angle(TokenKind::Greater),
            TokenKind::GreaterEqual => self.split_close_angle(TokenKind::Equal),
            _ => {
                let span = self.current_span();
                let message = format!("expected `>` to close generics, found {}", self.peek());
                self.error(span, message);
                span
            }
        }
    }

    /// Split the leading `>` off the current two-character token, leaving
    /// `remainder` as the current token.
    fn split_close_angle(&mut self, remainder: TokenKind) -> Span {
        let span = self.current.span;
        let split_at = span.start.saturating_add(1);
        self.current = Token::new(remainder, Span::new(split_at, span.end));
        Span::new(span.start, split_at)
    }

    fn parse_table_type(&mut self) -> Type {
        let open = self.advance_span(); // `{`
        let mut fields = Punctuated::<TypeField>::empty();

        while !matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
            fields.push(self.parse_type_field());
            let has_separator = matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon);
            if has_separator {
                self.advance_span();
            }
            fields.has_trailing_separator = has_separator;
            if !has_separator {
                break;
            }
        }

        // Grammar: `TableType ::= '{' Type '}' | '{' [PropList] '}'`. An array
        // table holds exactly one type and no separator, and a PropList holds
        // at most one indexer.
        let mut has_seen_indexer = false;
        for field in &fields.items {
            match field {
                TypeField::Indexer { span, key, .. } => {
                    // `["a"]: T` is a named field, as `parseTableType` builds it.
                    if let Type::Singleton(Token { kind: TokenKind::StringLiteral(text), .. }) = key
                    {
                        if decode_string(text, self.version).is_none_or(|bytes| bytes.contains(&0))
                        {
                            self.error(
                                *span,
                                "string literal contains malformed escape sequence or `\\0`",
                            );
                        }
                        continue;
                    }
                    if has_seen_indexer {
                        self.error(*span, "cannot have more than one table indexer");
                    }
                    has_seen_indexer = true;
                }
                TypeField::Array { span, .. } => {
                    if fields.len() > 1 {
                        self.error(
                            *span,
                            "an array-like table type holds exactly one element type, so use named fields or an indexer instead",
                        );
                        break;
                    }
                    if fields.has_trailing_separator {
                        self.error(*span, "an array-like table type takes no trailing separator");
                    }
                }
                TypeField::Named { .. } => {}
            }
        }

        let close = self.expect(&TokenKind::RightBrace);
        Type::Table(Box::new(TableType { span: open.merge(close), fields }))
    }

    fn parse_type_field(&mut self) -> TypeField {
        // `read`/`write` is a modifier unless `:` follows it; `{ read: number }`
        // has a field named `read`.
        let access = if let TokenKind::Identifier(name) = self.peek() {
            ((name == "read" || name == "write") && !matches!(self.peek_next(), TokenKind::Colon))
                .then(|| self.advance())
        } else {
            None
        };

        if matches!(self.peek(), TokenKind::LeftBracket) {
            let open = self.advance_span();
            let key = self.parse_type();
            self.expect(&TokenKind::RightBracket);
            self.expect(&TokenKind::Colon);
            let value = self.parse_type();
            let start_span = access.as_ref().map_or(open, |token| token.span);
            return TypeField::Indexer { span: start_span.merge(value.span()), access, key, value };
        }

        if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
            let name = self.advance();
            self.advance_span();
            let value = self.parse_type();
            let start_span = access.as_ref().map_or(name.span, |token| token.span);
            return TypeField::Named { span: start_span.merge(value.span()), access, name, value };
        }

        let value = self.parse_type();
        let start_span = access.as_ref().map_or_else(|| value.span(), |token| token.span);
        TypeField::Array { span: start_span.merge(value.span()), access, value }
    }

    /// Parse a parenthesized type `(T)`, a function type `(a: T, ...U) -> R`,
    /// or, when `is_pack_allowed`, a pack such as `()` or `(T, ...U)`. A `->`
    /// after the `)` decides which.
    fn parse_paren_or_function_type(&mut self, is_pack_allowed: bool) -> Type {
        let open = self.advance_span(); // `(`
        let mut params = Punctuated::<FunctionTypeParam>::empty();

        if !matches!(self.peek(), TokenKind::RightParen | TokenKind::Eof) {
            loop {
                // Grammar: `TypeList ::= Type [',' TypeList] | ...Type`, so a
                // pack ends the list.
                if self.is_at_type_pack() {
                    let type_value = self.parse_type_pack();
                    params.push(FunctionTypeParam {
                        span: type_value.span(),
                        name: None,
                        type_value,
                    });
                    break;
                }
                let param =
                    if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
                        let name = self.advance();
                        self.advance_span(); // `:`
                        let type_value = self.parse_type();
                        FunctionTypeParam {
                            span: name.span.merge(type_value.span()),
                            name: Some(name),
                            type_value,
                        }
                    } else {
                        let type_value = self.parse_type();
                        FunctionTypeParam { span: type_value.span(), name: None, type_value }
                    };
                let is_error = matches!(param.type_value, Type::Error(_));
                params.push(param);

                if !self.eat(&TokenKind::Comma) || is_error {
                    break;
                }
            }
        }

        let close = self.expect(&TokenKind::RightParen);

        if self.eat(&TokenKind::Arrow) {
            let return_type = self.parse_return_type();
            let span = open.merge(return_type.span());
            return Type::Function(Box::new(FunctionType {
                span,
                generics: None,
                params,
                return_type,
            }));
        }

        // No arrow, so this is a parenthesized type or a pack, and neither
        // takes parameter names.
        for param in params.iter() {
            if let Some(name) = &param.name {
                self.error(name.span, "named parameters are only valid in function types");
            }
        }

        let span = open.merge(close);
        // `(T)` is a parenthesized type; `()`, `(T, U)`, and `(...T)` are packs.
        let is_single_type = params.len() == 1
            && !params.first().is_some_and(|param| {
                matches!(param.type_value, Type::Variadic(_) | Type::GenericPack(_))
            });
        if is_single_type && let Some(param) = params.items.pop() {
            return Type::Parenthesized(Box::new(ParenthesizedType {
                span,
                type_value: param.type_value,
            }));
        }
        if !is_pack_allowed {
            if params.is_empty() {
                self.error(span, "expected `->` after `()`, did you mean `nil`?");
            } else {
                self.report_expected(&TokenKind::Arrow);
            }
            return Type::Error(span);
        }
        Type::Pack(Box::new(TypePack {
            span,
            types: Punctuated::from_items(
                params.items.into_iter().map(|param| param.type_value).collect(),
            ),
        }))
    }

    /// Parse the `(params) -> R` that follows a generic list.
    fn parse_function_type(&mut self, generics: GenericTypeList) -> Type {
        let start_span = generics.span;

        if !matches!(self.peek(), TokenKind::LeftParen) {
            let span = self.current_span();
            self.error(span, format!("expected `(` after generics, found {}", self.peek()));
            return Type::Error(start_span.merge(span));
        }

        let parsed = self.parse_paren_or_function_type(true);
        let Type::Function(mut function_type) = parsed else {
            // `<T>` followed by a paren type with no `->`, so the generics
            // have nothing to attach to.
            self.error(parsed.span(), "expected `->` after generic function type parameters");
            return parsed;
        };
        function_type.span = start_span.merge(function_type.span);
        function_type.generics = Some(generics);
        Type::Function(function_type)
    }
}
