//! Luau's guess at the `end` an unclosed construct is missing.
//!
//! It's the most recent construct inside it whose own `end` is indented apart
//! from the line it opens on, which suggests the `end` was written for the
//! construct around it.

use luck_ast::{Block, FunctionBody, GetSpan as _, Statement, Visit};
use luck_diagnostics::{Diagnostic, Label};
use luck_token::{Span, Token, TokenKind, has_line_break};

/// An error that a block is missing its `end` or `until`.
#[derive(Clone, Copy)]
pub(super) struct UnclosedBlock {
    /// Its index in the parse's errors.
    pub(super) error_index: usize,
    /// The token that opened the block.
    pub(super) opener: Span,
}

/// Labels each of `unclosed`'s errors with the construct `chunk` suggests
/// lost its `end`, if one does.
pub(super) fn label_forgotten_ends(
    chunk: &Block<'_>,
    source: &[u8],
    unclosed: &[UnclosedBlock],
    errors: &mut [Diagnostic],
) {
    // The tree keeps no span for a `function` or `until` keyword, so they
    // come from lexing the source again, which only a parse that left a
    // block unclosed pays for.
    let tokens = luck_lexer::lex(source).tokens;
    let mut finder = Misindented { source, tokens: &tokens, found: Vec::new() };
    finder.visit_block(chunk);
    for &UnclosedBlock { error_index, opener } in unclosed {
        let error = &mut errors[error_index];
        let at = error.span.expect("a parse error has a span").start;
        let latest = finder
            .found
            .iter()
            .filter(|suspect| opener.end <= suspect.opener.start && suspect.opener.start < at)
            .filter(|suspect| finder.is_on_another_line(opener, suspect.opener))
            .max_by_key(|suspect| suspect.opener.start);
        if let Some(suspect) = latest {
            let message = format!("did you forget to close this `{}`?", suspect.keyword);
            error.labels.push(Label { span: suspect.opener, message: message.into() });
        }
    }
}

/// A construct whose closer sits on another line than its keyword, at
/// another indentation.
struct Suspect {
    /// Its keyword's span.
    opener: Span,
    keyword: &'static str,
}

struct Misindented<'source> {
    source: &'source [u8],
    tokens: &'source [Token],
    found: Vec<Suspect>,
}

impl Misindented<'_> {
    /// Notes the construct `keyword` opens at `opener` when `closer`, the
    /// `end` or `until` that closes it, is a suspect.
    fn check(&mut self, keyword: &'static str, opener: Span, closer: Option<Span>) {
        let Some(closing) = closer else {
            return;
        };
        if self.is_on_another_line(opener, closing)
            && indentation(self.source, opener.start) != indentation(self.source, closing.start)
        {
            self.found.push(Suspect { opener, keyword });
        }
    }

    fn is_on_another_line(&self, earlier: Span, later: Span) -> bool {
        let between = self.source.get(earlier.end as usize..later.start as usize);
        between.is_some_and(has_line_break)
    }

    /// The last `keyword` token that starts at `from` or later and before
    /// `to`.
    fn last_keyword(&self, keyword: TokenKind, from: u32, to: u32) -> Option<Span> {
        let end = self.tokens.partition_point(|token| token.span.start < to);
        self.tokens[..end]
            .iter()
            .rev()
            .take_while(|token| token.span.start >= from)
            .find(|token| token.kind == keyword)
            .map(|token| token.span)
    }

    /// The `end` that `span`, a construct's, ends with, if it ends with one.
    fn end_of(&self, span: Span) -> Option<Span> {
        let end = Span::new(span.end.saturating_sub(3), span.end);
        (end.text(self.source) == b"end").then_some(end)
    }
}

impl<'ast> Visit<'ast> for Misindented<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        let span = stmt.span();
        let keyword = match stmt {
            Statement::IfStatement(_) => Some("if"),
            Statement::WhileLoop(_) => Some("while"),
            Statement::DoBlock(_) => Some("do"),
            Statement::NumericFor(_) | Statement::GenericFor(_) => Some("for"),
            Statement::RepeatLoop(repeat) => {
                let opener = Span::new(span.start, span.start + 6);
                // Past the block, so a nested loop's `until` isn't taken for
                // a missing one.
                let after_block =
                    repeat.block.stmts.last().map_or(opener.end, |last| last.span().end);
                let condition = repeat.condition.span().start;
                let until = self.last_keyword(TokenKind::Until, after_block, condition);
                self.check("repeat", opener, until);
                None
            }
            // A function's `end` closes its body, which `visit_function_body`
            // checks.
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::FunctionDecl(_)
            | Statement::LocalFunction(_)
            | Statement::LocalAssignment(_)
            | Statement::Return(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
            | Statement::Declaration(_)
            | Statement::Error(_) => None,
        };
        if let Some(word) = keyword {
            let start = span.start as usize;
            let opener = Span::from_indices(start, start + word.len());
            self.check(word, opener, self.end_of(span));
        }
        self.walk_stmt(stmt);
    }

    fn visit_function_body(&mut self, function_body: &'ast FunctionBody<'ast>) {
        // Only the function's name comes between its keyword and its body.
        if let Some(opener) = self.last_keyword(TokenKind::Function, 0, function_body.span.start) {
            self.check("function", opener, self.end_of(function_body.span));
        }
        self.walk_function_body(function_body);
    }
}

/// The spaces and tabs that open the line holding `offset`.
fn indentation(source: &[u8], offset: u32) -> &[u8] {
    let index = offset as usize;
    let line_start = source[..index].iter().rposition(|&b| b == b'\n').map_or(0, |at| at + 1);
    let line = &source[line_start..];
    let width = line.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    &line[..width]
}
