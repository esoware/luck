//! A definition file's `declare` statements, as Luau's `parseDeclaration`
//! reads them.

use luck_allocator::{Box, List};
use luck_ast::{
    DeclaredFunction, Expression, ExternTypeMember, FunctionAttribute, GetSpan as _, Statement,
    SuffixedExpression, Type, TypeField, TypeFieldAccess,
};
use luck_diagnostics::Diagnostic;
use luck_token::{Span, TokenKind};

use super::Parser;
use super::recovery::Closer;
use super::types::GenericsOwner;

/// What declares a function without a body, which decides what its
/// parameters take.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Declarer {
    /// `declare function`, which takes generics.
    Global,
    /// An extern type's `function`, whose first parameter is `self`.
    ExternType,
}

impl<'ast> Parser<'ast> {
    /// A statement that opens with `declare`, the current token, after the
    /// `attributes` before it, which only `declare function` takes.
    ///
    /// After attributes, Luau's `parseAttributeStat` reads a whole primary
    /// expression from `declare`, as `declare.x` or `declare(1)`, and drops
    /// it, where without them a suffix makes `declare` an ordinary name.
    pub(super) fn parse_declaration(
        &mut self,
        attributes: List<'ast, FunctionAttribute<'ast>>,
    ) -> Statement<'ast> {
        let declare = self.token.span;
        let start = attributes.first().map_or(declare, |attribute| attribute.span);
        let declare_chain = if attributes.is_empty() {
            self.advance();
            None
        } else {
            let outer = self.begin_operand();
            let primary = self.parse_primary_expr();
            self.end_operand(outer);
            if let Expression::Suffixed(chain) = primary { Some(chain) } else { None }
        };
        if self.eat(TokenKind::Function) {
            let function =
                self.parse_declared_function(start, attributes, declare_chain, Declarer::Global);
            return Statement::DeclaredFunction(function);
        }
        if !attributes.is_empty() {
            self.report_unexpected("expected `function` after attributes");
            return Statement::Error(start);
        }
        // Luau reads `extern` here as the extern type's keyword, whatever
        // follows it.
        if self.is_at_word(b"extern") {
            return self.parse_declared_extern_type(declare);
        }
        if !self.is_at_name() {
            self.report_unexpected("expected a name, `function`, or `extern type` after `declare`");
            return Statement::Error(declare);
        }
        let name = self.advance_name();
        self.expect(TokenKind::Colon);
        let ty = self.parse_declared_type();
        let span = Span::new(declare.start, self.previous_end);
        Statement::new_declared_global(span, name, ty, self.allocator)
    }

    /// A syntax error at the current token, which `expected` says what
    /// should have been.
    #[cold]
    #[inline(never)]
    fn report_unexpected(&mut self, expected: &str) {
        let span = self.token.span;
        let message = format!("{expected}, found {}", self.token.display(self.source));
        self.syntax_error(span, message);
    }

    /// The rest of a function declared without a body after its `function`
    /// keyword, from `start`.
    fn parse_declared_function(
        &mut self,
        start: Span,
        attributes: List<'ast, FunctionAttribute<'ast>>,
        declare_chain: Option<Box<'ast, SuffixedExpression<'ast>>>,
        declarer: Declarer,
    ) -> Box<'ast, DeclaredFunction<'ast>> {
        let name = self.expect_identifier_recover();
        let generics = (declarer == Declarer::Global && self.is_at(TokenKind::Less)).then(|| {
            let generics = self.parse_generic_type_list(GenericsOwner::Function);
            self.alloc(generics)
        });
        let open = self.expect(TokenKind::LeftParen);
        self.open(Closer::Paren);
        let (params, vararg) = self.parse_params();
        self.expect_closer(TokenKind::RightParen, Closer::Paren, open);
        let return_type = self.eat(TokenKind::Colon).then(|| {
            let ty = self.parse_return_type();
            self.alloc(ty)
        });
        let span = Span::new(start.start, self.previous_end);
        let function = DeclaredFunction::boxed(
            span,
            attributes,
            declare_chain,
            name,
            generics,
            params,
            vararg,
            return_type,
            self.allocator,
        );
        // After a syntax error in the signature, a missing annotation would
        // only echo it.
        if !self.is_recovering() {
            self.check_declared_params(&function, declarer);
        }
        function
    }

    /// Luau's type checker reads a declaration's types from its annotations,
    /// so its parser asks for one on every parameter but a method's `self`.
    fn check_declared_params(&mut self, function: &DeclaredFunction<'ast>, declarer: Declarer) {
        let (params, message) = match declarer {
            Declarer::Global => (&*function.params, "all declaration parameters must be annotated"),
            Declarer::ExternType => match function.params.split_first() {
                Some((first, rest))
                    if first.name.text == "self" && first.type_annotation.is_none() =>
                {
                    (rest, "all declaration parameters aside from `self` must be annotated")
                }
                other => {
                    let span = other.map_or(function.name.span, |(param, _)| param.span());
                    self.error(span, "`self` must be present as the unannotated first parameter");
                    return;
                }
            },
        };
        for param in params.iter().filter(|param| param.type_annotation.is_none()) {
            self.error(param.name.span, message);
        }
        if let Some(vararg) = &function.vararg
            && vararg.type_annotation.is_none()
        {
            self.error(vararg.span, message);
        }
    }

    /// `declare extern type Name extends Supertype with ... end`, at its
    /// `extern`.
    fn parse_declared_extern_type(&mut self, declare: Span) -> Statement<'ast> {
        self.advance(); // `extern`
        if !self.eat(TokenKind::Type) {
            self.report_unexpected("expected `type` after `extern`");
            return Statement::Error(declare);
        }
        let name = self.expect_identifier_recover();
        let supertype = self.is_at_word(b"extends").then(|| {
            self.advance();
            self.expect_identifier_recover()
        });
        // Luau reads the members on without one.
        if self.is_at_word(b"with") {
            self.advance();
        } else {
            self.report_unexpected("expected `with` before the extern type's members");
        }
        self.open(Closer::BlockEnd);
        let start = self.scratch.extern_type_members.len();
        let mut indexer = None;
        while !self.is_at_block_end() {
            self.parse_extern_type_member(&mut indexer);
        }
        let members =
            List::split_off_in(&mut self.scratch.extern_type_members, start, self.allocator);
        let end = self.expect_closer(TokenKind::End, Closer::BlockEnd, declare);
        let span = declare.merge(end);
        Statement::new_declared_extern_type(span, name, supertype, members, self.allocator)
    }

    /// Parse one member onto the extern type being parsed, or skip a token
    /// that opens none. `indexer` is where its indexer is, once it has one.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other token opens a property")]
    fn parse_extern_type_member(&mut self, indexer: &mut Option<Span>) {
        let member = match self.peek() {
            TokenKind::At => {
                let attributes = self.parse_function_attributes();
                let start = attributes.first().map_or(self.token.span, |attribute| attribute.span);
                if !self.eat(TokenKind::Function) {
                    self.report_unexpected("expected `function` after attributes");
                    return;
                }
                let method =
                    self.parse_declared_function(start, attributes, None, Declarer::ExternType);
                ExternTypeMember::Method(method)
            }
            TokenKind::Function => {
                let start = self.advance();
                let method =
                    self.parse_declared_function(start, List::new(), None, Declarer::ExternType);
                ExternTypeMember::Method(method)
            }
            TokenKind::LeftBracket => {
                let open = self.advance();
                self.open(Closer::Bracket);
                let key = self.parse_type();
                self.expect_closer(TokenKind::RightBracket, Closer::Bracket, open);
                self.expect(TokenKind::Colon);
                let value = self.parse_type();
                let span = open.merge(value.span());
                self.check_extern_type_key(&key, span, indexer);
                ExternTypeMember::Field(TypeField::Indexer { span, access: None, key, value })
            }
            _ => {
                let Some(field) = self.parse_extern_type_property() else {
                    return;
                };
                ExternTypeMember::Field(field)
            }
        };
        self.scratch.extern_type_members.push(member);
    }

    /// `["name"]: T` is a property, as in a table type, and any other key
    /// makes the indexer, of which there's one at most.
    fn check_extern_type_key(&mut self, key: &Type<'ast>, span: Span, indexer: &mut Option<Span>) {
        if let Type::StringLiteral(literal) = key {
            // The lexer reports a malformed escape on its own.
            if literal.value.contains(&0) {
                self.error(span, "string literal contains malformed escape sequence or `\\0`");
            }
        } else if let Some(first) = *indexer {
            let diag =
                Diagnostic::e008(span, "cannot have more than one indexer on an extern type")
                    .with_label(first, "first indexer here");
            self.errors.push(diag);
        } else {
            *indexer = Some(span);
        }
    }

    /// `[read | write] name: T`. A word that opens it is its access, which
    /// `read` and `write` are, unless `:` follows it.
    #[expect(clippy::wildcard_enum_match_arm, reason = "every other name is no access")]
    fn parse_extern_type_property(&mut self) -> Option<TypeField<'ast>> {
        let opener = self.token.span;
        let access = if self.is_at_name() && self.peek_next() != TokenKind::Colon {
            let access = match self.peek() {
                TokenKind::Read => Some(TypeFieldAccess::Read),
                TokenKind::Write => Some(TypeFieldAccess::Write),
                _ => {
                    self.syntax_error(opener, "a property's access must be `read` or `write`");
                    None
                }
            };
            self.advance();
            access
        } else {
            None
        };
        if !self.is_at_name() {
            self.report_unexpected("expected a property, an indexer, or a method");
            if !self.is_at_block_end() {
                self.advance();
            }
            return None;
        }
        let name = self.advance_name();
        self.expect(TokenKind::Colon);
        let value = self.parse_type();
        let start = if access.is_some() { opener } else { name.span };
        Some(TypeField::Named { span: start.merge(value.span()), access, name, value })
    }
}
