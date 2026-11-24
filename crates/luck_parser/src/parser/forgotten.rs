//! Luau's guess at the `end` an unclosed construct is missing.
//!
//! It's the most recent construct inside it whose own `end` is indented apart
//! from the line it opens on, which suggests the `end` was written for the
//! construct around it.

use luck_ast::{Chunk, FunctionBody, GetSpan as _, Statement, Visit};
use luck_diagnostics::{Diagnostic, Label};
use luck_token::{LuaVersion, Span, has_line_break};

/// Labels each of `unclosed`'s errors, an index into `errors` and the span of
/// the token that opened what it leaves open, with the construct `chunk`
/// suggests lost its `end`, if one does.
pub(super) fn label_forgotten_ends(
    chunk: &Chunk<'_>,
    source: &[u8],
    unclosed: &[(usize, Span)],
    errors: &mut [Diagnostic],
) {
    let mut finder = Misindented { source, version: chunk.version, found: Vec::new() };
    finder.visit_block(&chunk.block);
    for &(index, opener) in unclosed {
        let error = &mut errors[index];
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
    version: LuaVersion,
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
        between.is_some_and(|text| has_line_break(text, self.version))
    }

    /// The last `word` before `offset`, which a construct's keyword is when
    /// only a name, attributes, or whitespace come between.
    fn last_before(&self, word: &str, offset: u32) -> Option<Span> {
        let before = &self.source[..offset as usize];
        let start = before.windows(word.len()).rposition(|window| window == word.as_bytes())?;
        Some(Span::from_indices(start, start + word.len()))
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
                let until = self.last_before("until", repeat.condition.span().start);
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
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalFunction(_)
            | Statement::GlobalStar(_)
            | Statement::CompoundAssignment(_)
            | Statement::TypeDeclaration(_)
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
        if let Some(opener) = self.last_before("function", function_body.span.start) {
            self.check("function", opener, self.end_of(function_body.span));
        }
        self.walk_function_body(function_body);
    }
}

/// The spaces and tabs that open the line holding `offset`.
fn indentation(source: &[u8], offset: u32) -> &[u8] {
    let index = offset as usize;
    let line_start =
        source[..index].iter().rposition(|&b| b == b'\n' || b == b'\r').map_or(0, |at| at + 1);
    let line = &source[line_start..];
    let width = line.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
    &line[..width]
}
