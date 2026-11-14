//! Union (`|` and `?`) and intersection (`&`) suffixes can't mix
//! without parentheses. A `->` after the closing paren is what makes
//! `(params) -> R` a function type, and its return type may be a pack.

use luck_ast::{
    FunctionType, FunctionTypeParam, GenericPackType, GenericTypeList, GenericTypeParam,
    IntersectionType, NamedType, OptionalType, ParenthesizedType, Punctuated, TableType, Type,
    TypeArgs, TypeField, TypePack, TypeofType, UnionType, VariadicType,
};
use luck_token::{Span, Token, TokenKind, decode_string};

use super::skip::TypeExtent;
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
impl Parser<'_> {
    /// Parse `: T` if present, on a binding that takes a single type.
    pub(super) fn try_parse_type_annotation(&mut self) -> Option<Type> {
        (self.version.has_type_syntax() && self.eat(&TokenKind::Colon)).then(|| self.parse_type())
    }

    const fn is_at_type_pack(&self) -> bool {
        matches!(self.peek(), TokenKind::DotDotDot) || self.is_at_generic_pack()
    }

    const fn is_at_generic_pack(&self) -> bool {
        self.is_at_identifier() && matches!(self.peek_next(), TokenKind::DotDotDot)
    }

    fn parse_type_pack(&mut self) -> Type {
        if self.is_at_generic_pack() {
            return self.parse_generic_pack();
        }
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Full);
        }
        let dots = self.advance_span();
        self.enter_depth();
        let type_value = self.parse_type();
        self.exit_depth();
        let span = dots.merge(type_value.span());
        Type::Variadic(Box::new(VariadicType { span, type_value }))
    }

    fn parse_generic_pack(&mut self) -> Type {
        let name = self.advance();
        let dots = self.advance_span();
        let span = name.span.merge(dots);
        Type::GenericPack(Box::new(GenericPackType { span, name }))
    }

    pub(super) fn parse_vararg_type(&mut self) -> Type {
        if self.is_at_generic_pack() { self.parse_generic_pack() } else { self.parse_type() }
    }

    pub(super) fn parse_return_type(&mut self) -> Type {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !matches!(self.peek(), TokenKind::LeftParen) {
            return self.parse_type();
        }
        let level = self.nesting_depth;
        let outer = self.begin_operand();
        let mut first = self.parse_paren_or_function_type(true);
        // `parseReturnType` folds a suffix over `(T)` and over the `T` of
        // `(T, ...U)`, so `(A, B)?` and `() -> (A, B) | C` leave it unconsumed.
        // A refused one could have been either, so it takes the suffix too.
        if matches!(first, Type::Parenthesized(_) | Type::Error(_)) {
            first = self.parse_union_level(first);
        }
        if let Type::Pack(pack) = &mut first
            && let [element, Type::Variadic(_) | Type::GenericPack(_)] =
                pack.types.items.as_mut_slice()
        {
            // The suffix wraps an element one level inside the pack, and
            // counts from the whole pack's deepest level.
            self.nesting_depth = level.saturating_add(1);
            *element = self.parse_union_level(element.take());
            self.nesting_depth = level;
            pack.span = pack.span.merge(element.span());
        }
        self.end_operand(outer);
        first
    }

    fn parse_type_or_pack(&mut self) -> Type {
        if self.is_at_type_pack() {
            return self.parse_type_pack();
        }
        if !matches!(self.peek(), TokenKind::LeftParen) {
            return self.parse_type();
        }
        let outer = self.begin_operand();
        let first = self.parse_paren_or_function_type(true);
        let result =
            if matches!(first, Type::Pack(_)) { first } else { self.parse_union_level(first) };
        self.end_operand(outer);
        result
    }

    /// A pack is valid only where [`Self::parse_return_type`] or [`Self::parse_type_or_pack`] takes
    /// one.
    ///
    /// A type that holds others is a level of nesting, which each such type
    /// counts where it's built. A union, an intersection, or a `?` wraps a
    /// member already parsed, so it counts from that member's deepest level.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token takes the default path")]
    pub(super) fn parse_type(&mut self) -> Type {
        let outer = self.begin_operand();
        let result = match self.peek() {
            // A leading separator, as in the multiline definition style:
            // `type T =` newline `| A` newline `| B`.
            TokenKind::Pipe => self.parse_leading_pipe_union(),
            TokenKind::Ampersand => self.parse_leading_ampersand_intersection(),
            _ => {
                let primary = self.parse_primary_type();
                self.parse_union_level(primary)
            }
        };
        self.end_operand(outer);
        result
    }

    fn parse_leading_pipe_union(&mut self) -> Type {
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Full);
        }
        let leading_pipe = self.advance_span();
        self.enter_depth();
        let first = self.parse_union_member();
        let types = self.parse_union_members(first);
        self.exit_depth();
        let last_span = types.last().map_or(leading_pipe, Type::span);
        Type::Union(Box::new(UnionType {
            span: leading_pipe.merge(last_span),
            has_leading_pipe: true,
            types,
        }))
    }

    fn parse_leading_ampersand_intersection(&mut self) -> Type {
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Full);
        }
        let leading_ampersand = self.advance_span();
        self.enter_depth();
        let first = self.parse_intersection_member();
        let types = self.parse_intersection_members(first);
        self.exit_depth();
        let last_span = types.last().map_or(leading_ampersand, Type::span);
        Type::Intersection(Box::new(IntersectionType {
            span: leading_ampersand.merge(last_span),
            has_leading_ampersand: true,
            types,
        }))
    }

    fn parse_union_level(&mut self, primary: Type) -> Type {
        let level = self.nesting_depth;
        let first = self.parse_intersection_level(primary);
        if !matches!(self.peek(), TokenKind::Pipe) {
            return first;
        }
        if !self.enter_wrapper(level) {
            return self.refuse_type_wrapper(first.span(), level, TypeExtent::Full);
        }
        let start_span = first.span();
        let types = self.parse_union_members(first);
        self.nesting_depth = level;
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Union(Box::new(UnionType { span, has_leading_pipe: false, types }))
    }

    /// A member after the first, measured on its own.
    fn parse_union_member(&mut self) -> Type {
        let outer = self.begin_operand();
        let primary = self.parse_primary_type();
        let member = self.parse_intersection_level(primary);
        self.end_operand(outer);
        member
    }

    /// Luau's type suffix chain is flat, so `A & B | C` and `A | B & C` are
    /// both mixing errors, not a precedence question.
    fn parse_union_members(&mut self, first: Type) -> Punctuated<Type> {
        if matches!(first, Type::Intersection(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let mut members = vec![first];
        while self.eat(&TokenKind::Pipe) {
            let next = self.parse_union_member();
            if matches!(next, Type::Intersection(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            members.push(next);
        }
        Punctuated::from_items(members)
    }

    fn parse_intersection_level(&mut self, primary: Type) -> Type {
        let level = self.nesting_depth;
        let first = self.parse_postfix_type(primary);
        if !matches!(self.peek(), TokenKind::Ampersand) {
            return first;
        }
        if !self.enter_wrapper(level) {
            return self.refuse_type_wrapper(first.span(), level, TypeExtent::UnionMember);
        }
        let start_span = first.span();
        let types = self.parse_intersection_members(first);
        self.nesting_depth = level;
        let span = start_span.merge(types.last().map_or(start_span, Type::span));
        Type::Intersection(Box::new(IntersectionType { span, has_leading_ampersand: false, types }))
    }

    /// A member after the first, measured on its own.
    fn parse_intersection_member(&mut self) -> Type {
        let outer = self.begin_operand();
        let primary = self.parse_primary_type();
        let member = self.parse_postfix_type(primary);
        self.end_operand(outer);
        member
    }

    /// `?` is a union in Luau, so `A? & B` and `A & B?` are the same mixing
    /// error as `A | B & C`.
    fn parse_intersection_members(&mut self, first: Type) -> Punctuated<Type> {
        if matches!(first, Type::Optional(_)) {
            self.error(first.span(), MIXED_TYPE_MESSAGE);
        }
        let mut members = vec![first];
        while self.eat(&TokenKind::Ampersand) {
            let next = self.parse_intersection_member();
            if matches!(next, Type::Optional(_)) {
                self.error(next.span(), MIXED_TYPE_MESSAGE);
            }
            members.push(next);
        }
        Punctuated::from_items(members)
    }

    /// Each `?` nests the type one level deeper, so each one counts toward
    /// the depth cap from the type's deepest level.
    fn parse_postfix_type(&mut self, primary: Type) -> Type {
        let level = self.nesting_depth;
        let mut result = primary;
        // `?` stacks (`T??`), which Luau also accepts.
        while matches!(self.peek(), TokenKind::Question) {
            if !self.enter_wrapper(level) {
                return self.refuse_type_wrapper(
                    result.span(),
                    level,
                    TypeExtent::IntersectionMember,
                );
            }
            self.nesting_depth = level;
            let question = self.advance_span();
            let span = result.span().merge(question);
            result = Type::Optional(Box::new(OptionalType { span, type_value: result }));
        }
        result
    }

    /// Refuse a `?`, intersection, or union wrapping the type from `start` at
    /// `level`, skipping the rest of the type that `extent` of it takes.
    #[cold]
    #[inline(never)]
    fn refuse_type_wrapper(&mut self, start: Span, level: usize, extent: TypeExtent) -> Type {
        self.refuse_wrapper(level);
        self.skip_type_tail(extent);
        Type::Error(start.merge(self.previous_span))
    }

    /// Refuse the type at the current token, skipping as much of it as
    /// `extent` takes.
    #[cold]
    #[inline(never)]
    fn refuse_type(&mut self, extent: TypeExtent) -> Type {
        let start = self.current_span();
        self.report_depth_limit(start);
        self.skip_type(extent);
        Type::Error(start.merge(self.previous_span))
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
                if matches!(self.peek_next(), TokenKind::Dot | TokenKind::Less) && !self.can_nest(1)
                {
                    return self.refuse_type(TypeExtent::Primary);
                }
                let name = self.advance();
                self.parse_named_type(name)
            }
            // Singletons. Luau has no number singleton, but accepting one
            // keeps sources that carry it round-tripping.
            TokenKind::Nil
            | TokenKind::True
            | TokenKind::False
            | TokenKind::StringLiteral(_)
            | TokenKind::Number(_)
            | TokenKind::Integer(_) => Type::Singleton(self.advance()),
            TokenKind::LeftBrace => self.parse_table_type(),
            TokenKind::LeftParen => self.parse_paren_or_function_type(false),
            // `<T>(...) -> R` is a generic function type.
            TokenKind::Less => self.parse_generic_function_type(),
            _ => {
                let span = self.current_span();
                let message = format!("expected type, found {}", self.peek());
                self.syntax_error(span, message);
                // Don't consume the token; it may close an enclosing construct
                // or start the next statement.
                Type::Error(span)
            }
        }
    }

    fn parse_typeof_type(&mut self) -> Type {
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Primary);
        }
        let typeof_span = self.advance_span();
        if !matches!(self.peek(), TokenKind::LeftParen) {
            self.report_expected(&TokenKind::LeftParen);
            return Type::Error(typeof_span);
        }
        self.enter_depth();
        self.advance_span();
        self.open(Closer::Paren);
        let expr = self.parse_expr(0);
        self.exit_depth();
        let close = self.expect_closer(&TokenKind::RightParen, Closer::Paren);
        let span = typeof_span.merge(close);
        Type::Typeof(Box::new(TypeofType { span, expr }))
    }

    /// The caller has checked that a qualified or generic name fits.
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
        self.enter_depth();
        let (prefix, name) = if self.eat(&TokenKind::Dot) {
            let name = self.expect_identifier_recover();
            (Some(first_name), name)
        } else {
            (None, first_name)
        };

        let generics = matches!(self.peek(), TokenKind::Less).then(|| self.parse_type_args());
        self.exit_depth();

        let start_span = prefix.as_ref().map_or(name.span, |module| module.span);
        let end_span = generics.as_ref().map_or(name.span, |args| args.span);
        Type::Named(Box::new(NamedType {
            span: start_span.merge(end_span),
            prefix,
            name,
            generics,
        }))
    }

    pub(super) fn parse_type_args(&mut self) -> TypeArgs {
        let open = self.advance_span(); // `<`
        let mut args = Punctuated::<Type>::empty();

        if !matches!(
            self.peek(),
            TokenKind::Greater | TokenKind::ShiftRight | TokenKind::GreaterEqual | TokenKind::Eof
        ) {
            loop {
                let start = self.current_span().start;
                let arg = self.parse_type_or_pack();
                // A refused argument is skipped whole, so only an error that
                // read nothing ends the list.
                let has_read = self.previous_span.end > start;
                args.push(arg);
                if !self.eat(&TokenKind::Comma) || !has_read {
                    break;
                }
            }
        }

        let close = self.consume_type_close_angle();
        TypeArgs { span: open.merge(close), args }
    }

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
            params.push(param);

            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let close = self.consume_type_close_angle();
        GenericTypeList { span: open.merge(close), params }
    }

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
                self.syntax_error(span, message);
                span
            }
        }
    }

    fn parse_table_type(&mut self) -> Type {
        if !self.can_nest(1) {
            return Type::Error(self.refuse_group());
        }
        let open = self.advance_span(); // `{`
        self.enter_depth();
        self.open(Closer::Brace);
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
        self.exit_depth();

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

        let close = self.expect_closer(&TokenKind::RightBrace, Closer::Brace);
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
            return self.parse_indexer_type_field(access);
        }
        if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
            return self.parse_named_type_field(access);
        }

        let value = self.parse_type();
        let start_span = access.as_ref().map_or_else(|| value.span(), |token| token.span);
        TypeField::Array { span: start_span.merge(value.span()), access, value }
    }

    fn parse_indexer_type_field(&mut self, access: Option<Token>) -> TypeField {
        let open = self.advance_span();
        self.open(Closer::Bracket);
        let key = self.parse_type();
        self.expect_closer(&TokenKind::RightBracket, Closer::Bracket);
        self.expect(&TokenKind::Colon);
        let value = self.parse_type();
        let start_span = access.as_ref().map_or(open, |token| token.span);
        TypeField::Indexer { span: start_span.merge(value.span()), access, key, value }
    }

    fn parse_named_type_field(&mut self, access: Option<Token>) -> TypeField {
        let name = self.advance();
        self.advance_span();
        let value = self.parse_type();
        let start_span = access.as_ref().map_or(name.span, |token| token.span);
        TypeField::Named { span: start_span.merge(value.span()), access, name, value }
    }

    /// Parse a parenthesized type `(T)`, a function type `(a: T, ...U) -> R`,
    /// or, when `is_pack_allowed`, a pack such as `()` or `(T, ...U)`. A `->`
    /// after the `)` decides which.
    fn parse_paren_or_function_type(&mut self, is_pack_allowed: bool) -> Type {
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Primary);
        }
        self.enter_depth();
        let parsed = self.parse_paren_or_function_members(is_pack_allowed);
        self.exit_depth();
        parsed
    }

    /// The caller has counted the level all three forms take.
    fn parse_paren_or_function_members(&mut self, is_pack_allowed: bool) -> Type {
        let open = self.advance_span(); // `(`
        self.open(Closer::Paren);
        let params = self.parse_function_type_params();
        let close = self.expect_closer(&TokenKind::RightParen, Closer::Paren);

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
        self.finish_paren_or_pack_type(open.merge(close), params, is_pack_allowed)
    }

    fn parse_function_type_params(&mut self) -> Punctuated<FunctionTypeParam> {
        let mut params = Punctuated::<FunctionTypeParam>::empty();
        if matches!(self.peek(), TokenKind::RightParen | TokenKind::Eof) {
            return params;
        }
        loop {
            // Grammar: `TypeList ::= Type [',' TypeList] | ...Type`, so a
            // pack ends the list.
            let is_pack = self.is_at_type_pack();
            let start = self.current_span().start;
            let param = self.parse_function_type_param(is_pack);
            // A refused parameter is skipped whole, so only an error that
            // read nothing ends the list.
            let has_read = self.previous_span.end > start;
            params.push(param);
            if is_pack || !self.eat(&TokenKind::Comma) || !has_read {
                return params;
            }
        }
    }

    fn parse_function_type_param(&mut self, is_pack: bool) -> FunctionTypeParam {
        if is_pack {
            let type_value = self.parse_type_pack();
            return FunctionTypeParam { span: type_value.span(), name: None, type_value };
        }
        if self.is_at_identifier() && matches!(self.peek_next(), TokenKind::Colon) {
            let name = self.advance();
            self.advance_span(); // `:`
            let type_value = self.parse_type();
            return FunctionTypeParam {
                span: name.span.merge(type_value.span()),
                name: Some(name),
                type_value,
            };
        }
        let type_value = self.parse_type();
        FunctionTypeParam { span: type_value.span(), name: None, type_value }
    }

    /// No arrow followed, so this is a parenthesized type or a pack, and
    /// neither takes parameter names.
    fn finish_paren_or_pack_type(
        &mut self,
        span: Span,
        mut params: Punctuated<FunctionTypeParam>,
        is_pack_allowed: bool,
    ) -> Type {
        for param in params.iter() {
            if let Some(name) = &param.name {
                self.error(name.span, "named parameters are only valid in function types");
            }
        }

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
                self.syntax_error(span, "expected `->` after `()`, did you mean `nil`?");
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

    fn parse_generic_function_type(&mut self) -> Type {
        if !self.can_nest(1) {
            return self.refuse_type(TypeExtent::Primary);
        }
        self.enter_depth();
        let generics = self.parse_generic_type_list(GenericsOwner::Function);
        let parsed = self.parse_function_type(generics);
        self.exit_depth();
        parsed
    }

    fn parse_function_type(&mut self, generics: GenericTypeList) -> Type {
        let start_span = generics.span;

        if !matches!(self.peek(), TokenKind::LeftParen) {
            let span = self.current_span();
            let message = format!("expected `(` after generics, found {}", self.peek());
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
        function_type.generics = Some(generics);
        Type::Function(function_type)
    }
}
