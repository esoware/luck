use core::mem;

use luck_ast::{Block, FunctionBody, Parameter, Punctuated, Type, VarArgParam};
use luck_token::{Token, TokenKind};

use super::types::GenericsOwner;
use super::{FunctionScope, Parser};

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the parser is one type with its grammar split by file"
)]
impl Parser<'_> {
    pub(super) fn parse_function_body(&mut self) -> FunctionBody {
        if !self.enter_depth() {
            let span = self.current_span();
            return FunctionBody {
                span,
                generics: None,
                params: Punctuated::empty(),
                vararg: None,
                return_type: None,
                block: Block { span, stmts: Vec::new(), last_stmt: None },
            };
        }

        // Luau puts the generic list before the parens: `function f<T>(x: T)`.
        let generics = (self.version.has_type_syntax() && matches!(self.peek(), TokenKind::Less))
            .then(|| Box::new(self.parse_generic_type_list(GenericsOwner::Function)));

        let open = self.expect(&TokenKind::LeftParen);
        let start_span = generics.as_ref().map_or(open, |list| list.span);

        let (params, vararg) = self.parse_params();

        self.expect(&TokenKind::RightParen);

        // Luau: optional return type annotation after `)`.
        let return_type = (self.version.has_type_syntax() && self.eat(&TokenKind::Colon))
            .then(|| self.parse_return_type());

        // A loop around the function doesn't make `break` or `continue` valid
        // inside it, and `...` needs this function's own vararg parameter.
        let outer_scope = mem::replace(
            &mut self.function_scope,
            FunctionScope { is_chunk: false, is_vararg: vararg.is_some(), is_in_loop: false },
        );
        let block = self.parse_block();
        self.function_scope = outer_scope;

        let end_span = self.expect(&TokenKind::End);

        self.exit_depth();

        let span = start_span.merge(end_span);

        FunctionBody { span, generics, params, vararg, return_type, block }
    }

    /// Parse the parameter list between a function's parens, stopping before
    /// the closing `)`.
    fn parse_params(&mut self) -> (Punctuated<Parameter>, Option<Box<VarArgParam>>) {
        let mut params = Punctuated::<Parameter>::empty();
        if matches!(self.peek(), TokenKind::DotDotDot) {
            return (params, Some(Box::new(self.parse_vararg_param())));
        }
        if !self.is_at_identifier() {
            return (params, None);
        }

        let first_name = self.advance();
        params.push(self.parse_parameter(first_name));

        while self.eat(&TokenKind::Comma) {
            if matches!(self.peek(), TokenKind::DotDotDot) {
                // Vararg after the last named parameter.
                return (params, Some(Box::new(self.parse_vararg_param())));
            }
            let name = self.expect_identifier_recover();
            params.push(self.parse_parameter(name));
        }
        (params, None)
    }

    /// Finish a function parameter or a for-loop variable whose `name` was
    /// just consumed, taking a Luau `: T` annotation if one follows.
    pub(super) fn parse_parameter(&mut self, name: Token) -> Parameter {
        let type_annotation = self.try_parse_type_annotation();
        let end_span = type_annotation.as_ref().map_or(name.span, Type::span);
        Parameter { span: name.span.merge(end_span), name, type_annotation }
    }

    /// Parse `... [name] [: type]`; `...` must be the current token.
    fn parse_vararg_param(&mut self) -> VarArgParam {
        let dots = self.advance_span();
        // Lua 5.5: named varargs `...name`.
        let vararg_name =
            (self.version.has_named_varargs() && self.is_at_identifier()).then(|| self.advance());
        // Luau: vararg type annotation `...: type` or `...: T...`.
        let vararg_type = (self.version.has_type_syntax() && self.eat(&TokenKind::Colon))
            .then(|| self.parse_vararg_type());
        let end_span = vararg_type
            .as_ref()
            .map(Type::span)
            .or_else(|| vararg_name.as_ref().map(|name| name.span))
            .unwrap_or(dots);
        VarArgParam { span: dots.merge(end_span), name: vararg_name, type_annotation: vararg_type }
    }
}
