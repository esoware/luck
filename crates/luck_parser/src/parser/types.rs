//! Union (`|` and `?`) and intersection (`&`) suffixes can't mix
//! without parentheses. A `->` after the closing paren is what makes
//! `(params) -> R` a function type, and its return type may be a pack.

use core::mem;

use luck_allocator::List;
use luck_ast::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, Name, NamedType, OptionalType, ParenthesizedType, TableType, Type, TypeArgs,
    TypeField, TypeFieldAccess, TypePack, TypeofType, UnionType, VariadicType,
};
use luck_token::{Span, Token, TokenKind, decode_string};

use super::{Closer, Parser};

/// Luau's flat type-suffix chain rejects `A | B & C`, `A? & B`, and the like.
const MIXED_TYPE_MESSAGE: &str =
    "mixing union and intersection types is not allowed, consider wrapping in parentheses";

/// What declares a generic parameter list, which decides whether its
/// parameters may take defaults.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum GenericsOwner {
    TypeAlias,
    Function,
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl<'ast> Parser<'ast> {
    /// Parse `: T` if present, on a binding that takes a single type.
    #[inline]
    pub(super) fn try_parse_type_annotation(&mut self) -> Option<Type<'ast>> {
        (self.version.has_type_syntax() && self.eat(TokenKind::Colon)).then(|| self.parse_type())
    }

    fn is_at_type_pack(&self) -> bool {
        self.is_at(TokenKind::DotDotDot) || self.is_at_generic_pack()
    }

    fn is_at_generic_pack(&self) -> bool {
        self.is_at_identifier() && self.peek_next() == TokenKind::DotDotDot
    }

    fn parse_type_pack(&mut self) -> Type<'ast> {
        if self.is_at_generic_pack() {
            return self.parse_generic_pack();
        }
        let dots = self.token.span;
        if !self.enter_depth() {
            return Type::Error(dots);
        }
        self.advance();
        let ty = self.parse_type();
        self.exit_depth();
        let span = dots.merge(ty.span());
        Type::Variadic(self.alloc(VariadicType { span, ty }))
    }

    fn parse_generic_pack(&mut self) -> Type<'ast> {
        let name = self.advance_name();
        let dots = self.advance_span();
        let span = name.span.merge(dots);
        Type::GenericPack(self.alloc(GenericPackType { span, name }))
    }

    pub(super) fn parse_vararg_type(&mut self) -> Type<'ast> {
        if self.is_at_generic_pack() { self.parse_generic_pack() } else { self.parse_type() }
    }

    pub(super) fn parse_return_type(&mut self) -> Type<'ast> {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !self.is_at(TokenKind::LeftParen) {
            return self.parse_type();
        }
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let mut first = self.parse_paren_or_function_type(true);
        // `parseReturnType` folds a suffix over `(T)` and over the `T` of
        // `(T, ...U)`, so `(A, B)?` and `() -> (A, B) | C` leave it unconsumed.
        if matches!(first, Type::Parenthesized(_)) {
            first = self.parse_union_level(first);
        }
        if let Type::Pack(pack) = &mut first
            && let [element, Type::Variadic(_) | Type::GenericPack(_)] = &mut *pack.types
        {
            // The suffix wraps an element one level inside the pack, and
            // counts from the whole pack's deepest level.
            self.nesting_depth = level.wrapping_add(1);
            *element = self.parse_union_level(element.take());
            self.nesting_depth = level;
            let element_span = element.span();
            pack.span = pack.span.merge(element_span);
        }
        self.end_operand(outer);
        first
    }

    fn parse_type_or_pack(&mut self) -> Type<'ast> {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !self.is_at(TokenKind::LeftParen) {
            return self.parse_type();
        }
        let outer = self.begin_operand();
        let first = self.parse_paren_or_function_type(true);
        let result =
            if matches!(first, Type::Pack(_)) { first } else { self.parse_union_level(first) };
        self.end_operand(outer);
        result
    }

    /// A pack is valid only where [`Self::parse_return_type`] or
    /// [`Self::parse_type_or_pack`] takes one.
    ///
    /// A type that holds others is a level of nesting, which each such type
    /// counts where it's built. A union, an intersection, or a `?` wraps a
    /// member already parsed, so it counts from that member's deepest level.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_type(&mut self) -> Type<'ast> {
        let outer = self.begin_operand();
        let result = match self.peek() {
            // A leading separator, as in the multiline definition style:
            // `type T =` newline `| A` newline `| B`.
            TokenKind::Pipe => self.parse_leading_separator_type(true),
            TokenKind::Ampersand => self.parse_leading_separator_type(false),
            _ => {
                let primary = self.parse_primary_type();
                self.parse_union_level(primary)
            }
        };
        self.end_operand(outer);
        result
    }

    /// A union after a leading `|`, or an intersection after a leading `&`.
    #[inline(never)]
    fn parse_leading_separator_type(&mut self, is_union: bool) -> Type<'ast> {
        let leading = self.token.span;
        if !self.enter_depth() {
            return Type::Error(leading);
        }
        self.advance();
        let ty = if is_union {
            let first = self.parse_union_member();
            let types = self.parse_union_members(first);
            let span = leading.merge(types.last().map_or(leading, Type::span));
            Type::Union(self.alloc(UnionType { span, has_leading_pipe: true, types }))
        } else {
            let first = self.parse_intersection_member();
            let types = self.parse_intersection_members(first);
            let span = leading.merge(types.last().map_or(leading, Type::span));
            Type::Intersection(self.alloc(IntersectionType {
                span,
                has_leading_ampersand: true,
                types,
            }))
        };
        self.exit_depth();
        ty
    }

    fn parse_union_level(&mut self, primary: Type<'ast>) -> Type<'ast> {
        let level = self.nesting_depth;
        let first = self.parse_intersection_level(primary);
        if !self.is_at(TokenKind::Pipe) {
            return first;
        }
        if !self.enter_wrapper(level) {
            return Type::Error(first.span());
        }
        let start_span = first.span();
        let types = self.parse_union_members(first);
        self.nesting_depth = level;
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Union(self.alloc(UnionType { span, has_leading_pipe: false, types }))
    }

    /// A member after the first, measured on its own.
    fn parse_union_member(&mut self) -> Type<'ast> {
        let outer = self.begin_operand();
        let primary = self.parse_primary_type();
        let member = self.parse_intersection_level(primary);
        self.end_operand(outer);
        member
    }

    /// Luau's type suffix chain is flat, so `A & B | C` and `A | B & C` are
    /// both mixing errors, not a precedence question.
    fn parse_union_members(&mut self, first: Type<'ast>) -> List<'ast, Type<'ast>> {
        if matches!(first, Type::Intersection(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let start = self.scratch.types.len();
        self.scratch.types.push(first);
        while self.eat(TokenKind::Pipe) {
            let next = self.parse_union_member();
            if matches!(next, Type::Intersection(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            self.scratch.types.push(next);
        }
        List::split_off_in(&mut self.scratch.types, start, self.allocator)
    }

    fn parse_intersection_level(&mut self, primary: Type<'ast>) -> Type<'ast> {
        let level = self.nesting_depth;
        let first = self.parse_postfix_type(primary);
        if !self.is_at(TokenKind::Ampersand) {
            return first;
        }
        if !self.enter_wrapper(level) {
            return Type::Error(first.span());
        }
        let start_span = first.span();
        let types = self.parse_intersection_members(first);
        self.nesting_depth = level;
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Intersection(self.alloc(IntersectionType {
            span,
            has_leading_ampersand: false,
            types,
        }))
    }

    /// A member after the first, measured on its own.
    fn parse_intersection_member(&mut self) -> Type<'ast> {
        let outer = self.begin_operand();
        let primary = self.parse_primary_type();
        let member = self.parse_postfix_type(primary);
        self.end_operand(outer);
        member
    }

    /// `?` is a union in Luau, so `A? & B` and `A & B?` are the same mixing
    /// error as `A | B & C`.
    fn parse_intersection_members(&mut self, first: Type<'ast>) -> List<'ast, Type<'ast>> {
        if matches!(first, Type::Optional(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let start = self.scratch.types.len();
        self.scratch.types.push(first);
        while self.eat(TokenKind::Ampersand) {
            let next = self.parse_intersection_member();
            if matches!(next, Type::Optional(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            self.scratch.types.push(next);
        }
        List::split_off_in(&mut self.scratch.types, start, self.allocator)
    }

    /// `?` stacks (`T??`), which Luau also accepts, each one a level.
    #[inline]
    fn parse_postfix_type(&mut self, primary: Type<'ast>) -> Type<'ast> {
        if self.is_at(TokenKind::Question) { self.parse_optionals(primary) } else { primary }
    }

    #[inline(never)]
    fn parse_optionals(&mut self, primary: Type<'ast>) -> Type<'ast> {
        let level = self.nesting_depth;
        let mut result = primary;
        while self.is_at(TokenKind::Question) {
            if !self.enter_wrapper(level) {
                return Type::Error(result.span());
            }
            self.nesting_depth = level;
            let question = self.advance_span();
            let span = result.span().merge(question);
            result = Type::Optional(self.alloc(OptionalType { span, ty: result }));
        }
        result
    }

    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn parse_primary_type(&mut self) -> Type<'ast> {
        match self.peek() {
            TokenKind::Identifier => {
                // `typeof.Name` is a module-qualified type.
                if self.current_text() == "typeof" && self.peek_next() != TokenKind::Dot {
                    return self.parse_typeof_type();
                }
                let name = self.advance_name();
                self.parse_named_type(name)
            }
            TokenKind::Nil => Type::Nil(self.advance_span()),
            TokenKind::True => Type::True(self.advance_span()),
            TokenKind::False => Type::False(self.advance_span()),
            TokenKind::StringLiteral => Type::StringLiteral(self.advance_literal()),
            // Luau has no number singleton, but accepting one keeps sources
            // that carry it round-tripping.
            TokenKind::Number | TokenKind::Integer => Type::Number(self.advance_literal()),
            TokenKind::LeftBrace => self.parse_table_type(),
            TokenKind::LeftParen => self.parse_paren_or_function_type(false),
            // `<T>(...) -> R` is a generic function type.
            TokenKind::Less => {
                let open = self.token.span;
                if !self.enter_depth() {
                    return Type::Error(open);
                }
                let generics = self.parse_generic_type_list(GenericsOwner::Function);
                let parsed = self.parse_function_type(generics);
                self.exit_depth();
                parsed
            }
            _ => self.report_expected_type(),
        }
    }

    /// The token stays in place; it may close an enclosing construct or
    /// start the next statement.
    #[cold]
    #[inline(never)]
    fn report_expected_type(&mut self) -> Type<'ast> {
        let span = self.token.span;
        let message = format!("expected type, found {}", self.token.display(self.source));
        self.syntax_error(span, message);
        Type::Error(span)
    }

    #[inline(never)]
    fn parse_typeof_type(&mut self) -> Type<'ast> {
        let typeof_span = self.advance_span();
        if !self.is_at(TokenKind::LeftParen) {
            self.report_expected(TokenKind::LeftParen);
            return Type::Error(typeof_span);
        }
        if !self.enter_depth() {
            return Type::Error(typeof_span);
        }
        self.advance();
        self.open(Closer::Paren);
        let expr = self.parse_expr(0);
        self.exit_depth();
        let close = self.expect_closer(TokenKind::RightParen, Closer::Paren);
        let span = typeof_span.merge(close);
        Type::Typeof(self.alloc(TypeofType { span, expr }))
    }

    fn parse_named_type(&mut self, first_name: Name<'ast>) -> Type<'ast> {
        if !matches!(self.peek(), TokenKind::Dot | TokenKind::Less) {
            return Type::Name(self.alloc(first_name));
        }
        if !self.enter_depth() {
            return Type::Error(first_name.span);
        }
        let (prefix, name) = if self.eat(TokenKind::Dot) {
            (Some(first_name), self.expect_identifier_recover())
        } else {
            (None, first_name)
        };
        let generics = self.is_at(TokenKind::Less).then(|| self.parse_type_args());
        self.exit_depth();
        let start_span = prefix.map_or(name.span, |module| module.span);
        let end_span = generics.as_ref().map_or(name.span, |args| args.span);
        let span = start_span.merge(end_span);
        Type::Named(self.alloc(NamedType { span, prefix, name, generics }))
    }

    pub(super) fn parse_type_args(&mut self) -> TypeArgs<'ast> {
        let open = self.advance_span(); // `<`
        let start = self.scratch.types.len();
        if !matches!(self.peek(), TokenKind::Greater | TokenKind::GreaterEqual | TokenKind::Eof) {
            loop {
                let arg = self.parse_type_or_pack();
                let is_error = matches!(arg, Type::Error(_));
                self.scratch.types.push(arg);
                if !self.eat(TokenKind::Comma) || is_error {
                    break;
                }
            }
        }
        let args = List::split_off_in(&mut self.scratch.types, start, self.allocator);
        let close = self.consume_type_close_angle();
        TypeArgs { span: open.merge(close), args }
    }

    pub(super) fn parse_generic_type_list(
        &mut self,
        owner: GenericsOwner,
    ) -> GenericTypeList<'ast> {
        let open = self.advance_span(); // `<`
        let start = self.scratch.generic_params.len();
        let mut has_seen_pack = false;
        let mut has_seen_default = false;

        while let Some(name) = self.expect_identifier() {
            let dots = self.is_at(TokenKind::DotDotDot).then(|| self.advance_span());
            let default = self.eat(TokenKind::Equal).then(|| {
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
                    // `(T)` in a pack position is a one-type pack, and an error
                    // has been reported already.
                    let is_pack_default = matches!(
                        default_type,
                        Type::Pack(_)
                            | Type::Variadic(_)
                            | Type::GenericPack(_)
                            | Type::Parenthesized(_)
                            | Type::Error(_)
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
            self.scratch.generic_params.push(param);

            if !self.eat(TokenKind::Comma) {
                break;
            }
        }

        let params = List::split_off_in(&mut self.scratch.generic_params, start, self.allocator);
        let close = self.consume_type_close_angle();
        GenericTypeList { span: open.merge(close), params }
    }

    /// The lexer is greedy, so `Foo<T>=x` holds `GreaterEqual`, which gets
    /// split here: this returns the `>`'s span and leaves `=` as the current
    /// token.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    fn consume_type_close_angle(&mut self) -> Span {
        match self.peek() {
            TokenKind::Greater => self.advance_span(),
            TokenKind::GreaterEqual => {
                let span = self.token.span;
                let split_at = span.start.saturating_add(1);
                self.token = Token {
                    span: Span::new(split_at, span.end),
                    kind: TokenKind::Equal,
                    has_line_break_before: false,
                    is_unterminated: false,
                };
                self.previous_span = Span::new(span.start, split_at);
                self.previous_span
            }
            _ => self.report_unclosed_generics(),
        }
    }

    #[cold]
    #[inline(never)]
    fn report_unclosed_generics(&mut self) -> Span {
        let span = self.token.span;
        let message =
            format!("expected `>` to close generics, found {}", self.token.display(self.source));
        self.syntax_error(span, message);
        span
    }

    #[inline(never)]
    fn parse_table_type(&mut self) -> Type<'ast> {
        let open = self.token.span;
        if !self.enter_depth() {
            return Type::Error(open);
        }
        self.advance(); // `{`
        self.open(Closer::Brace);
        let start = self.scratch.type_fields.len();
        let mut has_trailing_separator = false;
        while !matches!(self.peek(), TokenKind::RightBrace | TokenKind::Eof) {
            self.parse_type_field();
            has_trailing_separator = matches!(self.peek(), TokenKind::Comma | TokenKind::Semicolon);
            if !has_trailing_separator {
                break;
            }
            self.advance();
        }
        self.exit_depth();
        let fields = List::split_off_in(&mut self.scratch.type_fields, start, self.allocator);
        self.check_table_type_fields(&fields, has_trailing_separator);
        let close = self.expect_closer(TokenKind::RightBrace, Closer::Brace);
        let span = open.merge(close);
        Type::Table(self.alloc(TableType { span, fields, has_trailing_separator }))
    }

    /// Grammar: `TableType ::= '{' Type '}' | '{' [PropList] '}'`. An array
    /// table holds exactly one type and no separator, and a `PropList` holds
    /// at most one indexer.
    fn check_table_type_fields(
        &mut self,
        fields: &[TypeField<'ast>],
        has_trailing_separator: bool,
    ) {
        let mut has_seen_indexer = false;
        for field in fields {
            match field {
                TypeField::Indexer { span, key, .. } => {
                    // `["a"]: T` is a named field, as `parseTableType` builds it.
                    if let Type::StringLiteral(literal) = key {
                        if decode_string(literal.text, self.version)
                            .is_none_or(|bytes| bytes.contains(&0))
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
                    if has_trailing_separator {
                        self.error(*span, "an array-like table type takes no trailing separator");
                    }
                }
                TypeField::Named { .. } => {}
            }
        }
    }

    /// Parse one field onto the table type being parsed.
    fn parse_type_field(&mut self) {
        // `read`/`write` is a modifier unless `:` follows it; `{ read: number }`
        // has a field named `read`.
        let access = if self.is_at_identifier() && self.peek_next() != TokenKind::Colon {
            match self.current_text() {
                "read" => Some(TypeFieldAccess::Read),
                "write" => Some(TypeFieldAccess::Write),
                _ => None,
            }
        } else {
            None
        };
        let access_span = access.map(|_| self.advance_span());

        let field = if self.is_at(TokenKind::LeftBracket) {
            let open = self.advance_span();
            self.open(Closer::Bracket);
            let key = self.parse_type();
            self.expect_closer(TokenKind::RightBracket, Closer::Bracket);
            self.expect(TokenKind::Colon);
            let value = self.parse_type();
            let span = access_span.unwrap_or(open).merge(value.span());
            TypeField::Indexer { span, access, key, value }
        } else if self.is_at_identifier() && self.peek_next() == TokenKind::Colon {
            let name = self.advance_name();
            self.advance();
            let value = self.parse_type();
            let span = access_span.unwrap_or(name.span).merge(value.span());
            TypeField::Named { span, access, name, value }
        } else {
            let value = self.parse_type();
            let span = access_span.unwrap_or_else(|| value.span()).merge(value.span());
            TypeField::Array { span, access, value }
        };
        self.scratch.type_fields.push(field);
    }

    /// Parse a parenthesized type `(T)`, a function type `(a: T, ...U) -> R`,
    /// or, when `is_pack_allowed`, a pack such as `()` or `(T, ...U)`. A `->`
    /// after the `)` decides which.
    #[inline(never)]
    fn parse_paren_or_function_type(&mut self, is_pack_allowed: bool) -> Type<'ast> {
        let open = self.token.span;
        if !self.enter_depth() {
            return Type::Error(open);
        }
        let parsed = self.parse_paren_or_function_members(is_pack_allowed);
        self.exit_depth();
        parsed
    }

    /// The caller has counted the level all three forms take.
    fn parse_paren_or_function_members(&mut self, is_pack_allowed: bool) -> Type<'ast> {
        let open = self.advance_span(); // `(`
        self.open(Closer::Paren);
        let start = self.scratch.function_type_params.len();

        if !matches!(self.peek(), TokenKind::RightParen | TokenKind::Eof) {
            loop {
                // Grammar: `TypeList ::= Type [',' TypeList] | ...Type`, so a
                // pack ends the list.
                if self.is_at_type_pack() {
                    let ty = self.parse_type_pack();
                    self.scratch.function_type_params.push(FunctionTypeParam { name: None, ty });
                    break;
                }
                let name =
                    (self.is_at_identifier() && self.peek_next() == TokenKind::Colon).then(|| {
                        let name = self.advance_name();
                        self.advance(); // `:`
                        name
                    });
                let ty = self.parse_type();
                let is_error = matches!(ty, Type::Error(_));
                self.scratch.function_type_params.push(FunctionTypeParam { name, ty });
                if !self.eat(TokenKind::Comma) || is_error {
                    break;
                }
            }
        }

        let close = self.expect_closer(TokenKind::RightParen, Closer::Paren);

        if self.eat(TokenKind::Arrow) {
            let params =
                List::split_off_in(&mut self.scratch.function_type_params, start, self.allocator);
            let return_type = self.parse_return_type();
            let span = open.merge(return_type.span());
            return Type::Function(self.alloc(FunctionType {
                span,
                generics: None,
                params,
                return_type,
            }));
        }
        self.finish_paren_or_pack(open.merge(close), start, is_pack_allowed)
    }

    /// No arrow followed the members from `start`, so they're a
    /// parenthesized type or a pack, and neither takes parameter names.
    fn finish_paren_or_pack(
        &mut self,
        span: Span,
        start: usize,
        is_pack_allowed: bool,
    ) -> Type<'ast> {
        let mut params = mem::take(&mut self.scratch.function_type_params);
        for param in params.get(start..).unwrap_or_default() {
            if let Some(name) = param.name {
                self.error(name.span, "named parameters are only valid in function types");
            }
        }
        // `(T)` is a parenthesized type; `()`, `(T, U)`, and `(...T)` are packs.
        let ty = if let [single] = params.get(start..).unwrap_or_default()
            && !matches!(single.ty, Type::Variadic(_) | Type::GenericPack(_))
            && let Some(param) = params.pop()
        {
            Type::Parenthesized(self.alloc(ParenthesizedType { span, ty: param.ty }))
        } else if is_pack_allowed {
            let types_start = self.scratch.types.len();
            self.scratch.types.extend(params.drain(start..).map(|param| param.ty));
            let types = List::split_off_in(&mut self.scratch.types, types_start, self.allocator);
            Type::Pack(self.alloc(TypePack { span, types }))
        } else {
            let is_empty = params.len() == start;
            params.truncate(start);
            self.scratch.function_type_params = params;
            if is_empty {
                self.syntax_error(span, "expected `->` after `()`, did you mean `nil`?");
            } else {
                self.report_expected(TokenKind::Arrow);
            }
            return Type::Error(span);
        };
        self.scratch.function_type_params = params;
        ty
    }

    fn parse_function_type(&mut self, generics: GenericTypeList<'ast>) -> Type<'ast> {
        let start_span = generics.span;
        if !self.is_at(TokenKind::LeftParen) {
            let span = self.token.span;
            let message =
                format!("expected `(` after generics, found {}", self.token.display(self.source));
            self.syntax_error(span, message);
            return Type::Error(start_span.merge(span));
        }
        let parsed = self.parse_paren_or_function_members(true);
        let Type::Function(mut function_type) = parsed else {
            // `<T>` followed by a paren type with no `->`, so the generics
            // have nothing to attach to.
            self.syntax_error(
                parsed.span(),
                "expected `->` after generic function type parameters",
            );
            return parsed;
        };
        function_type.span = start_span.merge(function_type.span);
        function_type.generics = Some(self.alloc(generics));
        Type::Function(function_type)
    }
}
