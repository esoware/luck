//! Cursor-resolution helpers. Locate the identifier or expression at a
//! given byte offset by walking the AST. Used by hover, signature help,
//! document highlight, and the code-action provider.

use luck_ast::expr::{Expression, FieldAccess, FunctionCall, Var};
use luck_ast::shared::Block;
use luck_ast::visitor::Visitor;
use luck_token::{Span, Token, TokenKind};

fn token_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Identifier(name) => Some(name.as_str()),
        _ => None,
    }
}

/// The dotted name chain a callee/prefix resolves to, outermost segment
/// first, with each segment's name span. A chain broken by an index
/// expression, a call, or a non-name prefix yields only the trailing
/// segments that are plain names.
struct DottedChain {
    segments: Vec<String>,
    spans: Vec<Span>,
}

impl DottedChain {
    /// Unwind a `Var::FieldAccess` (`a.b.c`) into its name chain.
    fn from_field_access(access: &FieldAccess) -> Self {
        let mut segments = Vec::new();
        let mut spans = Vec::new();
        if let Some(name) = token_name(&access.name) {
            segments.push(name.to_string());
            spans.push(access.name.span);
        } else {
            return Self { segments, spans };
        }
        let mut prefix: &Expression = &access.prefix;
        loop {
            match prefix {
                Expression::Var(Var::Name(token)) => {
                    if let Some(name) = token_name(token) {
                        segments.push(name.to_string());
                        spans.push(token.span);
                    }
                    break;
                }
                Expression::Var(Var::FieldAccess(inner)) => {
                    let Some(name) = token_name(&inner.name) else {
                        break;
                    };
                    segments.push(name.to_string());
                    spans.push(inner.name.span);
                    prefix = &inner.prefix;
                }
                _ => break,
            }
        }
        segments.reverse();
        spans.reverse();
        Self { segments, spans }
    }

    /// The name chain a call's non-method callee resolves to, plus the span
    /// of its root name (for shape/shadow resolution).
    fn from_callee(callee: &Expression) -> Self {
        match callee {
            Expression::Var(Var::Name(token)) => {
                let mut chain = Self {
                    segments: Vec::new(),
                    spans: Vec::new(),
                };
                if let Some(name) = token_name(token) {
                    chain.segments.push(name.to_string());
                    chain.spans.push(token.span);
                }
                chain
            }
            Expression::Var(Var::FieldAccess(access)) => Self::from_field_access(access),
            _ => Self {
                segments: Vec::new(),
                spans: Vec::new(),
            },
        }
    }

    fn root_span(&self) -> Option<Span> {
        self.spans.first().copied()
    }
}

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
        /// Span of the root `Name` token, when the callee is a plain
        /// dotted chain - lets consumers resolve shaped/shadowed roots.
        base_span: Option<Span>,
        full_span: Span,
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

    fn record_field_access(&mut self, access: &FieldAccess) {
        let chain = DottedChain::from_field_access(access);
        if chain.segments.is_empty() {
            return;
        }
        self.try_set(CursorTarget::DottedPath {
            segments: chain.segments,
            spans: chain.spans,
            full_span: access.span,
        });
    }
}

impl<'ast> Visitor<'ast> for TargetFinder {
    fn visit_var(&mut self, var: &'ast Var) {
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

    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.record_call(call);
        }
        self.walk_expression(expr);
    }
}

impl TargetFinder {
    fn record_call(&mut self, call: &FunctionCall) {
        let chain = DottedChain::from_callee(&call.callee);
        let mut base_span = chain.root_span();
        let mut path = chain.segments;
        // The method name is a path segment too: `game:GetService(...)`
        // resolves as ["game", "GetService"] through shaped lookup.
        if let Some(method_token) = &call.method {
            match token_name(method_token) {
                Some(name) if !path.is_empty() => path.push(name.to_string()),
                _ => {
                    path.clear();
                    base_span = None;
                }
            }
        }

        self.try_set(CursorTarget::Call {
            path,
            base_span,
            full_span: call.span,
        });
    }
}

/// Find an enclosing call expression for `offset` plus the index of the
/// argument the cursor is inside (counting commas). Used by signature help.
#[derive(Debug, Clone)]
pub struct CallSite<'ast> {
    /// The call node itself, so consumers can resolve the callee
    /// semantically (shaped locals, shadow checks) instead of by text.
    pub call: &'ast FunctionCall,
    pub path: Vec<String>,
    pub active_param: u32,
    pub paren_span: Span,
}

#[must_use]
pub fn find_call_site_at<'ast>(
    block: &'ast Block,
    source: &str,
    offset: u32,
) -> Option<CallSite<'ast>> {
    let mut finder = CallSiteFinder {
        offset,
        source,
        result: None,
    };
    finder.visit_block(block);
    finder.result
}

struct CallSiteFinder<'src, 'ast> {
    offset: u32,
    source: &'src str,
    result: Option<CallSite<'ast>>,
}

impl<'ast> Visitor<'ast> for CallSiteFinder<'_, 'ast> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.try_record(call);
        }
        self.walk_expression(expr);
    }

    // Statement-position calls dispatch to walk_function_call without
    // an expression visit, so hook them here too.
    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            self.try_record(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

impl<'ast> CallSiteFinder<'_, 'ast> {
    fn try_record(&mut self, call: &'ast FunctionCall) {
        let span = call.span;
        if self.offset < span.start || self.offset > span.end {
            return;
        }
        // Heuristic: treat the first '(' after the callee as the open paren.
        let bytes = self.source.as_bytes();
        let scan_start = call.callee.span().end as usize;
        let scan_end = (span.end as usize).min(bytes.len());
        let Some(paren_offset) = bytes[scan_start..scan_end]
            .iter()
            .position(|&byte| byte == b'(')
        else {
            return;
        };
        let paren_open = (scan_start + paren_offset) as u32;
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
            call,
            path,
            active_param: commas,
            paren_span: Span::new(paren_open, span.end),
        });
    }
}

fn call_path(call: &FunctionCall) -> Vec<String> {
    let mut path = DottedChain::from_callee(&call.callee).segments;
    // The method name is a path segment too, so signature help resolves
    // `game:GetService(` through shaped lookup.
    if let Some(method_token) = &call.method {
        match token_name(method_token) {
            Some(name) if !path.is_empty() => path.push(name.to_string()),
            _ => path.clear(),
        }
    }
    path
}

/// A string-literal argument enclosing the cursor: the call it belongs
/// to, its positional index, and the literal's token span. Used by
/// completion to offer constant-set values inside the quotes.
#[derive(Debug, Clone, Copy)]
pub struct StringArgSite<'ast> {
    pub call: &'ast FunctionCall,
    pub arg_index: usize,
    pub literal_span: Span,
}

/// Find the innermost call whose argument list contains a string
/// literal enclosing `offset` (cursor strictly inside the quotes).
#[must_use]
pub fn find_string_arg_at(block: &Block, offset: u32) -> Option<StringArgSite<'_>> {
    let mut finder = StringArgFinder {
        offset,
        result: None,
    };
    finder.visit_block(block);
    finder.result
}

struct StringArgFinder<'ast> {
    offset: u32,
    result: Option<StringArgSite<'ast>>,
}

impl<'ast> Visitor<'ast> for StringArgFinder<'ast> {
    fn visit_expression(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.try_record(call);
        }
        self.walk_expression(expr);
    }

    // Statement-position calls dispatch to walk_function_call without
    // an expression visit, so hook them here too.
    fn visit_statement(&mut self, stmt: &'ast luck_ast::Statement) {
        if let luck_ast::Statement::FunctionCall(call_stmt) = stmt {
            self.try_record(&call_stmt.call);
        }
        self.walk_statement(stmt);
    }
}

impl<'ast> StringArgFinder<'ast> {
    /// Strictly inside the quotes so a cursor next to the literal does
    /// not trigger value completion.
    fn contains(&self, span: Span) -> bool {
        self.offset > span.start && self.offset < span.end
    }

    fn try_record(&mut self, call: &'ast FunctionCall) {
        let literal = match &call.args {
            luck_ast::expr::FunctionArgs::Parenthesized { args, .. } => {
                args.iter().enumerate().find_map(|(idx, arg)| match arg {
                    Expression::StringLiteral(token) if self.contains(token.span) => {
                        Some((idx, token.span))
                    }
                    _ => None,
                })
            }
            luck_ast::expr::FunctionArgs::StringLiteral(token) if self.contains(token.span) => {
                Some((0, token.span))
            }
            _ => None,
        };
        let Some((arg_index, span)) = literal else {
            return;
        };
        // Nested calls visit outer-first; the innermost match wins.
        self.result = Some(StringArgSite {
            call,
            arg_index,
            literal_span: span,
        });
    }
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
