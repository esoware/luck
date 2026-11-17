use core::mem;

use luck_allocator::{Box, List};
use luck_ast::{FunctionBody, Name, Parameter, Type, VarArgParam};
use luck_token::TokenKind;

use super::types::GenericsOwner;
use super::{Closer, FunctionScope, Parser};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl<'ast> Parser<'ast> {
    /// False when the body or its block would pass the depth cap. It parses
    /// into a body the caller's node already holds, so no recursive frame
    /// holds one.
    #[inline(never)]
    pub(super) fn parse_function_body(&mut self, body: &mut FunctionBody<'ast>) -> bool {
        if !self.enter_depth() {
            return false;
        }
        self.open(Closer::BlockEnd);

        // Luau puts the generic list before the parens: `function f<T>(x: T)`.
        if self.is_at(TokenKind::Less) && self.version.has_type_syntax() {
            let generics = self.parse_generic_type_list(GenericsOwner::Function);
            body.generics = Some(self.alloc(generics));
        }

        let open = self.expect(TokenKind::LeftParen);
        let start_span = body.generics.as_ref().map_or(open, |list| list.span);

        self.open(Closer::Paren);
        self.parse_params(body);
        self.expect_closer(TokenKind::RightParen, Closer::Paren);

        // Luau: optional return type annotation after `)`.
        if self.version.has_type_syntax() && self.eat(TokenKind::Colon) {
            body.return_type = Some(self.parse_return_type());
        }

        // A loop around the function doesn't make `break` or `continue` valid
        // inside it, and `...` needs this function's own vararg parameter.
        let outer_scope = mem::replace(
            &mut self.function_scope,
            FunctionScope { is_chunk: false, is_vararg: body.vararg.is_some(), is_in_loop: false },
        );
        let is_parsed = self.parse_block(&mut body.block);
        self.function_scope = outer_scope;
        self.exit_depth();
        if !is_parsed {
            return false;
        }

        self.close(Closer::BlockEnd);
        let end_span = self.expect(TokenKind::End);
        body.span = start_span.merge(end_span);
        true
    }

    /// Parse the parameter list between a function's parens into `body`,
    /// stopping before the closing `)`.
    fn parse_params(&mut self, body: &mut FunctionBody<'ast>) {
        if self.is_at(TokenKind::DotDotDot) {
            body.vararg = Some(self.parse_vararg_param());
            return;
        }
        if !self.is_at_identifier() {
            return;
        }
        let start = self.scratch.params.len();
        let first_name = self.advance_name();
        let first = self.parse_parameter(first_name);
        self.scratch.params.push(first);
        while self.eat(TokenKind::Comma) {
            if self.is_at(TokenKind::DotDotDot) {
                // Vararg after the last named parameter.
                body.vararg = Some(self.parse_vararg_param());
                break;
            }
            let name = self.expect_identifier_recover();
            let param = self.parse_parameter(name);
            self.scratch.params.push(param);
        }
        body.params = List::split_off_in(&mut self.scratch.params, start, self.allocator);
    }

    /// Finish a function parameter or a for-loop variable whose `name` was
    /// just consumed, taking a Luau `: T` annotation if one follows.
    pub(super) fn parse_parameter(&mut self, name: Name<'ast>) -> Parameter<'ast> {
        Parameter { name, type_annotation: self.try_parse_type_annotation() }
    }

    /// Parse `... [name] [: type]`; `...` must be the current token.
    fn parse_vararg_param(&mut self) -> Box<'ast, VarArgParam<'ast>> {
        let dots = self.advance();
        // Lua 5.5: named varargs `...name`.
        let name = (self.version.has_named_varargs() && self.is_at_identifier())
            .then(|| self.advance_name());
        // Luau: vararg type annotation `...: type` or `...: T...`.
        let type_annotation = (self.version.has_type_syntax() && self.eat(TokenKind::Colon))
            .then(|| self.parse_vararg_type());
        let end_span = type_annotation
            .as_ref()
            .map(Type::span)
            .or_else(|| name.map(|vararg_name| vararg_name.span))
            .unwrap_or(dots);
        self.alloc(VarArgParam { span: dots.merge(end_span), name, type_annotation })
    }
}
