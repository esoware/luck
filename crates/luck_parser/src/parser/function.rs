use core::mem;

use luck_allocator::{Box, List};
use luck_ast::{FunctionBody, GetSpan as _, Name, Parameter, Type, VarArgParam};
use luck_token::{Span, TokenKind};

use super::types::GenericsOwner;
use super::{Closer, FunctionScope, Parser};

impl<'ast> Parser<'ast> {
    /// The body after the `function` keyword at `function_span`, or `None`
    /// when the body or its block would pass the depth cap.
    pub(super) fn parse_function_body(
        &mut self,
        function_span: Span,
    ) -> Option<FunctionBody<'ast>> {
        if !self.enter_depth() {
            return None;
        }
        self.open(Closer::BlockEnd);

        // Luau puts the generic list before the parens: `function f<T>(x: T)`.
        let generics = (self.is_at(TokenKind::Less) && self.version.has_type_syntax()).then(|| {
            let generics = self.parse_generic_type_list(GenericsOwner::Function);
            self.alloc(generics)
        });

        let open = self.expect(TokenKind::LeftParen);
        let start_span = generics.as_ref().map_or(open, |list| list.span);

        self.open(Closer::Paren);
        let (params, vararg) = self.parse_params();
        self.expect_closer(TokenKind::RightParen, Closer::Paren, open);

        // Luau: optional return type annotation after `)`.
        let return_type =
            (self.version.has_type_syntax() && self.eat(TokenKind::Colon)).then(|| {
                let ty = self.parse_return_type();
                self.alloc(ty)
            });

        // A loop around the function doesn't make `break` or `continue` valid
        // inside it, and `...` needs this function's own vararg parameter.
        let outer_scope = mem::replace(
            &mut self.function_scope,
            FunctionScope { is_chunk: false, is_vararg: vararg.is_some(), is_in_loop: false },
        );
        let parsed_block = self.parse_block();
        self.function_scope = outer_scope;
        self.exit_depth();
        let block = parsed_block?;

        let end_span = self.expect_closer(TokenKind::End, Closer::BlockEnd, function_span);
        let span = start_span.merge(end_span);
        Some(FunctionBody::new(span, generics, params, vararg, return_type, block))
    }

    /// Parse the parameter list between a function's parens, stopping before
    /// the closing `)`.
    fn parse_params(
        &mut self,
    ) -> (List<'ast, Parameter<'ast>>, Option<Box<'ast, VarArgParam<'ast>>>) {
        if self.is_at(TokenKind::DotDotDot) {
            return (List::new(), Some(self.parse_vararg_param()));
        }
        if !self.is_at_name() {
            return (List::new(), None);
        }
        let start = self.scratch.params.len();
        let first_name = self.advance_name();
        let first = self.parse_parameter(first_name);
        self.scratch.params.push(first);
        let mut vararg = None;
        while self.eat(TokenKind::Comma) {
            if self.is_at(TokenKind::DotDotDot) {
                // Vararg after the last named parameter.
                vararg = Some(self.parse_vararg_param());
                break;
            }
            let name = self.expect_identifier_recover();
            let param = self.parse_parameter(name);
            self.scratch.params.push(param);
        }
        (List::split_off_in(&mut self.scratch.params, start, self.allocator), vararg)
    }

    /// Finish a function parameter or a for-loop variable whose `name` was
    /// just consumed, taking a Luau `: T` annotation if one follows.
    pub(super) fn parse_parameter(&mut self, name: Name<'ast>) -> Parameter<'ast> {
        self.counts.declarations = self.counts.declarations.wrapping_add(1);
        Parameter::new(name, self.try_parse_type_annotation())
    }

    /// Parse `... [name] [: type]`; `...` must be the current token.
    fn parse_vararg_param(&mut self) -> Box<'ast, VarArgParam<'ast>> {
        let dots = self.advance();
        // Lua 5.1: a vararg function binds its extra arguments' table to `arg`.
        if self.version.has_implicit_vararg_table() {
            self.counts.declarations = self.counts.declarations.wrapping_add(1);
        }
        // Lua 5.5: named varargs `...name`.
        let name = (self.version.has_named_varargs() && self.is_at_name()).then(|| {
            self.counts.declarations = self.counts.declarations.wrapping_add(1);
            self.advance_name()
        });
        // Luau: vararg type annotation `...: type` or `...: T...`.
        let type_annotation =
            (self.version.has_type_syntax() && self.eat(TokenKind::Colon)).then(|| {
                let ty = self.parse_vararg_type();
                self.alloc(ty)
            });
        let end_span = type_annotation
            .as_deref()
            .map(Type::span)
            .or_else(|| name.map(|vararg_name| vararg_name.span))
            .unwrap_or(dots);
        VarArgParam::boxed(dots.merge(end_span), name, type_annotation, self.allocator)
    }
}
