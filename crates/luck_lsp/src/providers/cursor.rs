//! Cursor-resolution helpers. Locate the identifier or expression at a
//! given byte offset by walking the AST. Used by hover, signature help,
//! document highlight, and the code-action provider.

#![allow(clippy::while_let_loop, clippy::needless_range_loop)]

use luck_ast::expr::{Expression, FieldAccess, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::{Span, Token, TokenKind};

/// What we found at the cursor.
#[derive(Debug, Clone)]
pub enum CursorTarget {
    /// A bare identifier (variable read/write or unresolved name).
    Identifier { name: String, span: Span },
    /// A dotted path like `string.format`. `segments` are name-only tokens.
    DottedPath {
        segments: Vec<String>,
        spans: Vec<Span>,
        full_span: Span,
    },
    /// A function call expression. Path is empty for indirect callees.
    Call {
        path: Vec<String>,
        full_span: Span,
        call_span: Span,
        is_method: bool,
    },
}

impl CursorTarget {
    /// Convenience: the textual path segments (single entry for plain identifier).
    #[must_use]
    pub fn path(&self) -> Vec<&str> {
        match self {
            CursorTarget::Identifier { name, .. } => vec![name.as_str()],
            CursorTarget::DottedPath { segments, .. } => {
                segments.iter().map(String::as_str).collect()
            }
            CursorTarget::Call { path, .. } => path.iter().map(String::as_str).collect(),
        }
    }

    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            CursorTarget::Identifier { span, .. } => *span,
            CursorTarget::DottedPath { full_span, .. } => *full_span,
            CursorTarget::Call { full_span, .. } => *full_span,
        }
    }
}

/// Find the smallest identifier-like AST construct whose span contains
/// `offset`. Returns `None` if the cursor lands on whitespace, an
/// operator, or a literal that is not name-like.
#[must_use]
pub fn find_target_at(block: &Block, offset: u32) -> Option<CursorTarget> {
    let mut finder = TargetFinder {
        offset,
        result: None,
    };
    finder.visit_block(block);
    finder.result
}

struct TargetFinder {
    offset: u32,
    result: Option<CursorTarget>,
}

impl TargetFinder {
    fn span_contains(&self, span: Span) -> bool {
        self.offset >= span.start && self.offset <= span.end
    }

    fn token_name(token: &Token) -> Option<String> {
        match &token.kind {
            TokenKind::Identifier(name) => Some(name.to_string()),
            _ => None,
        }
    }

    fn try_set(&mut self, target: CursorTarget) {
        let span = target.span();
        if !self.span_contains(span) {
            return;
        }
        let inner_size = span.end - span.start;
        match &self.result {
            Some(existing) => {
                let existing_span = existing.span();
                let existing_size = existing_span.end - existing_span.start;
                if inner_size <= existing_size {
                    self.result = Some(target);
                }
            }
            None => self.result = Some(target),
        }
    }

    fn record_field_access(&mut self, fa: &FieldAccess) {
        // Collect the dotted path by unwinding the prefix chain. We stop
        // at the first non-name prefix and record what we have.
        let mut segments: Vec<String> = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        if let Some(name) = Self::token_name(&fa.name) {
            segments.push(name);
            spans.push(fa.name.span);
        } else {
            return;
        }
        let mut cursor: &Expression = &fa.prefix;
        loop {
            match cursor {
                Expression::Var(var) => match var.as_ref() {
                    Var::Name(token) => {
                        if let Some(name) = Self::token_name(token) {
                            segments.push(name);
                            spans.push(token.span);
                        }
                        break;
                    }
                    Var::FieldAccess(inner) => {
                        if let Some(name) = Self::token_name(&inner.name) {
                            segments.push(name);
                            spans.push(inner.name.span);
                        } else {
                            break;
                        }
                        cursor = &inner.prefix;
                    }
                    Var::Index(_) => break,
                },
                _ => break,
            }
        }
        segments.reverse();
        spans.reverse();
        self.try_set(CursorTarget::DottedPath {
            segments,
            spans,
            full_span: fa.span,
        });
    }
}

impl Visitor for TargetFinder {
    fn visit_var(&mut self, var: &Var) {
        match var {
            Var::Name(token) => {
                if let TokenKind::Identifier(name) = &token.kind {
                    self.try_set(CursorTarget::Identifier {
                        name: name.to_string(),
                        span: token.span,
                    });
                }
            }
            Var::FieldAccess(fa) => {
                self.record_field_access(fa);
                self.walk_var(var);
            }
            Var::Index(_) => self.walk_var(var),
        }
    }

    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.record_call(call);
        }
        self.walk_expression(expr);
    }
}

impl TargetFinder {
    fn record_call(&mut self, call: &FunctionCall) {
        let mut path = Vec::new();
        let is_method = call.method.is_some();
        if let Expression::Var(var) = &call.callee {
            match var.as_ref() {
                Var::Name(token) => {
                    if let Some(name) = Self::token_name(token) {
                        path.push(name);
                    }
                }
                Var::FieldAccess(fa) => {
                    // Unwind prefix chain into path segments.
                    let mut segments: Vec<String> = Vec::new();
                    if let Some(name) = Self::token_name(&fa.name) {
                        segments.push(name);
                    }
                    let mut cursor: &Expression = &fa.prefix;
                    loop {
                        match cursor {
                            Expression::Var(inner_var) => match inner_var.as_ref() {
                                Var::Name(token) => {
                                    if let Some(name) = Self::token_name(token) {
                                        segments.push(name);
                                    }
                                    break;
                                }
                                Var::FieldAccess(inner_fa) => {
                                    if let Some(name) = Self::token_name(&inner_fa.name) {
                                        segments.push(name);
                                    } else {
                                        break;
                                    }
                                    cursor = &inner_fa.prefix;
                                }
                                Var::Index(_) => break,
                            },
                            _ => break,
                        }
                    }
                    segments.reverse();
                    path = segments;
                }
                Var::Index(_) => {}
            }
        }

        self.try_set(CursorTarget::Call {
            path,
            full_span: call.span,
            call_span: call.span,
            is_method,
        });
    }
}

/// Find an enclosing call expression for `offset` plus the index of the
/// argument the cursor is inside (counting commas). Used by signature help.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub path: Vec<String>,
    pub active_param: u32,
    pub paren_span: Span,
}

#[must_use]
pub fn find_call_site_at(block: &Block, source: &str, offset: u32) -> Option<CallSite> {
    let mut finder = CallSiteFinder {
        offset,
        source,
        result: None,
    };
    finder.visit_block(block);
    finder.result
}

struct CallSiteFinder<'src> {
    offset: u32,
    source: &'src str,
    result: Option<CallSite>,
}

impl Visitor for CallSiteFinder<'_> {
    fn visit_expression(&mut self, expr: &Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.try_record(call);
        }
        self.walk_expression(expr);
    }
}

impl CallSiteFinder<'_> {
    fn try_record(&mut self, call: &FunctionCall) {
        let span = call.span;
        if self.offset < span.start || self.offset > span.end {
            return;
        }
        // Heuristic: treat the first '(' after the callee as the open paren.
        let bytes = self.source.as_bytes();
        let mut paren_open: Option<u32> = None;
        for i in (call.callee.span().end as usize)..(span.end as usize).min(bytes.len()) {
            if bytes[i] == b'(' {
                paren_open = Some(i as u32);
                break;
            }
        }
        let Some(paren_open) = paren_open else { return };
        if self.offset <= paren_open {
            return;
        }
        // Active parameter = number of argument expressions that END
        // before the cursor. Spans come from the parser, so commas inside
        // strings, long brackets, and comments can never miscount (the
        // old byte rescanner was fooled by all three).
        let commas = match &call.args {
            luck_ast::expr::FunctionArgs::Parenthesized { args, .. } => {
                args.iter()
                    .filter(|arg| arg.span().end < self.offset)
                    .count() as u32
            }
            luck_ast::expr::FunctionArgs::StringLiteral(_)
            | luck_ast::expr::FunctionArgs::TableConstructor(_) => 0,
        };
        let path = call_path(call);
        self.result = Some(CallSite {
            path,
            active_param: commas,
            paren_span: Span::new(paren_open, span.end),
        });
    }
}

fn call_path(call: &FunctionCall) -> Vec<String> {
    let mut path = Vec::new();
    if let Expression::Var(var) = &call.callee {
        match var.as_ref() {
            Var::Name(token) => {
                if let TokenKind::Identifier(name) = &token.kind {
                    path.push(name.to_string());
                }
            }
            Var::FieldAccess(fa) => {
                let mut segments: Vec<String> = Vec::new();
                if let TokenKind::Identifier(name) = &fa.name.kind {
                    segments.push(name.to_string());
                }
                let mut cursor: &Expression = &fa.prefix;
                loop {
                    match cursor {
                        Expression::Var(inner_var) => match inner_var.as_ref() {
                            Var::Name(token) => {
                                if let TokenKind::Identifier(name) = &token.kind {
                                    segments.push(name.to_string());
                                }
                                break;
                            }
                            Var::FieldAccess(inner_fa) => {
                                if let TokenKind::Identifier(name) = &inner_fa.name.kind {
                                    segments.push(name.to_string());
                                } else {
                                    break;
                                }
                                cursor = &inner_fa.prefix;
                            }
                            Var::Index(_) => break,
                        },
                        _ => break,
                    }
                }
                segments.reverse();
                path = segments;
            }
            Var::Index(_) => {}
        }
    }
    path
}

/// Resolve the local symbol at a byte offset: a reference to it or its
/// declaration site. `None` for globals, keywords, and non-names -
/// span-exact resolution through the scope tree, so shadowed names map
/// to the right declaration (name matching cannot).
#[must_use]
pub fn symbol_at(
    tree: &luck_semantic::scope::ScopeTree,
    offset: u32,
) -> Option<luck_semantic::scope::SymbolId> {
    if let Some(reference) = tree
        .references
        .iter()
        .find(|reference| reference.span.start <= offset && offset <= reference.span.end)
    {
        return reference.resolved;
    }
    tree.symbols
        .iter()
        .find(|symbol| {
            symbol.definition_span.start <= offset && offset <= symbol.definition_span.end
        })
        .map(|symbol| symbol.id)
}

/// The exact identifier span at a byte offset: a reference span or a
/// declaration span. Used by rename to know what text to replace.
#[must_use]
pub fn name_span_at(tree: &luck_semantic::scope::ScopeTree, offset: u32) -> Option<Span> {
    if let Some(reference) = tree
        .references
        .iter()
        .find(|reference| reference.span.start <= offset && offset <= reference.span.end)
    {
        return Some(reference.span);
    }
    tree.symbols
        .iter()
        .find(|symbol| {
            symbol.definition_span.start <= offset && offset <= symbol.definition_span.end
        })
        .map(|symbol| symbol.definition_span)
}
