//! Lookups the providers share for what sits at a byte offset: the name or
//! call under the cursor, the enclosing call, a string argument, or a local
//! symbol.

use core::slice;

use luck_ast::{Block, Expression, FieldAccess, FunctionArgs, FunctionCall, Var, Visitor};
use luck_semantic::{ScopeTree, SymbolId};
use luck_token::{Span, index_to_offset, offset_to_index};

/// Whether `offset` falls inside `span`, either end included, so a cursor
/// just past a name still counts as on it.
pub(crate) const fn touches(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

/// The plain-name chain ending at `access` (`a.b.c`), root first, and the
/// root's span.
///
/// A chain broken by an index expression, a call, or a non-name prefix
/// yields only the trailing segments that are plain names. `None` when
/// `access` itself isn't named by an identifier.
fn field_access_chain(access: &FieldAccess) -> Option<(Vec<&str>, Span)> {
    let mut path = vec![access.name.identifier()?];
    let mut root_span = access.name.span;
    let mut prefix: &Expression = &access.prefix;
    while let Expression::Var(Var::FieldAccess(inner)) = prefix {
        let Some(name) = inner.name.identifier() else {
            break;
        };
        path.push(name);
        root_span = inner.name.span;
        prefix = &inner.prefix;
    }
    if let Expression::Var(Var::Name(token)) = prefix
        && let Some(name) = token.identifier()
    {
        path.push(name);
        root_span = token.span;
    }
    path.reverse();
    Some((path, root_span))
}

/// The name chain of a call's callee expression, not counting any `:method`
/// name, and the root's span.
fn callee_chain(callee: &Expression) -> Option<(Vec<&str>, Span)> {
    let Expression::Var(var) = callee else {
        return None;
    };
    match var {
        Var::Name(token) => Some((vec![token.identifier()?], token.span)),
        Var::FieldAccess(access) => field_access_chain(access),
        Var::Index(_) => None,
    }
}

/// What we found at the cursor.
#[derive(Debug)]
pub(crate) enum CursorNode<'ast> {
    /// A bare identifier (variable read/write or unresolved name).
    Identifier { name: &'ast str, span: Span },
    /// A dotted path like `string.format`, of name-only segments.
    DottedPath { path: Vec<&'ast str>, root_span: Span, full_span: Span },
    /// A function call expression. Path is empty for indirect callees.
    Call {
        path: Vec<&'ast str>,
        /// Span of the root `Name` token, when the callee is a plain
        /// dotted chain. Lets consumers resolve shaped/shadowed roots.
        root_span: Option<Span>,
        full_span: Span,
    },
}

impl<'ast> CursorNode<'ast> {
    /// The path segments; a single entry for a plain identifier.
    pub(crate) fn path(&self) -> &[&'ast str] {
        match self {
            Self::Identifier { name, .. } => slice::from_ref(name),
            Self::DottedPath { path, .. } | Self::Call { path, .. } => path,
        }
    }

    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Identifier { span, .. } => *span,
            Self::DottedPath { full_span, .. } | Self::Call { full_span, .. } => *full_span,
        }
    }
}

/// Find the smallest name, field access, or call expression whose span
/// contains `offset`.
pub(crate) fn find_node_at(block: &Block, offset: u32) -> Option<CursorNode<'_>> {
    let mut finder = NodeFinder { offset, result: None };
    finder.visit_block(block);
    finder.result
}

struct NodeFinder<'ast> {
    offset: u32,
    result: Option<CursorNode<'ast>>,
}

impl<'ast> NodeFinder<'ast> {
    fn try_set(&mut self, node: CursorNode<'ast>) {
        let span = node.span();
        if !touches(span, self.offset) {
            return;
        }
        if self.result.as_ref().is_none_or(|existing| span.len() <= existing.span().len()) {
            self.result = Some(node);
        }
    }

    fn record_call(&mut self, call: &'ast FunctionCall) {
        let (mut path, mut root_span) = callee_chain(&call.callee)
            .map_or_else(|| (Vec::new(), None), |(path, span)| (path, Some(span)));
        // The method name is a path segment too: `game:GetService(...)`
        // resolves as `["game", "GetService"]` through shaped lookup.
        if let Some(method_token) = &call.method {
            if let Some(name) = method_token.identifier()
                && !path.is_empty()
            {
                path.push(name);
            } else {
                path.clear();
                root_span = None;
            }
        }
        self.try_set(CursorNode::Call { path, root_span, full_span: call.span });
    }
}

impl<'ast> Visitor<'ast> for NodeFinder<'ast> {
    fn visit_var(&mut self, var: &'ast Var) {
        match var {
            Var::Name(token) => {
                if let Some(name) = token.identifier() {
                    self.try_set(CursorNode::Identifier { name, span: token.span });
                }
            }
            Var::FieldAccess(access) => {
                if let Some((path, root_span)) = field_access_chain(access) {
                    self.try_set(CursorNode::DottedPath {
                        path,
                        root_span,
                        full_span: access.span,
                    });
                }
                self.walk_var(var);
            }
            Var::Index(_) => self.walk_var(var),
        }
    }

    fn visit_expr(&mut self, expr: &'ast Expression) {
        if let Expression::FunctionCall(call) = expr {
            self.record_call(call);
        }
        self.walk_expr(expr);
    }
}

/// The call enclosing the cursor and the index of the argument the cursor is
/// in. Used by signature help.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallSite<'ast> {
    /// The call node, so consumers can resolve the callee semantically
    /// (shaped locals, shadow checks).
    pub call: &'ast FunctionCall,
    pub active_param: usize,
}

pub(crate) fn find_call_site_at<'ast>(
    block: &'ast Block,
    source: &str,
    offset: u32,
) -> Option<CallSite<'ast>> {
    let mut finder = CallSiteFinder { offset, source, result: None };
    finder.visit_block(block);
    finder.result
}

struct CallSiteFinder<'source, 'ast> {
    offset: u32,
    source: &'source str,
    result: Option<CallSite<'ast>>,
}

impl<'ast> Visitor<'ast> for CallSiteFinder<'_, 'ast> {
    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.try_record(call);
        self.walk_function_call(call);
    }
}

impl<'ast> CallSiteFinder<'_, 'ast> {
    fn try_record(&mut self, call: &'ast FunctionCall) {
        let span = call.span;
        if !touches(span, self.offset) {
            return;
        }
        // Heuristic: treat the first '(' after the callee as the open paren.
        let bytes = self.source.as_bytes();
        let callee_end = call.callee.span().end;
        let scan_end = offset_to_index(span.end).min(bytes.len());
        let Some(paren_offset) = bytes
            .get(offset_to_index(callee_end)..scan_end)
            .and_then(|scanned| scanned.iter().position(|&byte| byte == b'('))
        else {
            return;
        };
        let paren_open = callee_end.saturating_add(index_to_offset(paren_offset));
        if self.offset <= paren_open {
            return;
        }
        // The active parameter is the number of arguments that end before
        // the cursor. Parser spans can't be fooled by commas inside strings,
        // long brackets, or comments.
        let active_param = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                args.iter().filter(|arg| arg.span().end < self.offset).count()
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => 0,
        };
        self.result = Some(CallSite { call, active_param });
    }
}

/// A string-literal argument enclosing the cursor. Completion offers
/// constant-set values inside its quotes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StringArgSite<'ast> {
    pub call: &'ast FunctionCall,
    pub arg_index: usize,
    pub literal_span: Span,
}

/// Find the innermost call whose argument list contains a string
/// literal enclosing `offset` (cursor strictly inside the quotes).
pub(crate) fn find_string_arg_at(block: &Block, offset: u32) -> Option<StringArgSite<'_>> {
    let mut finder = StringArgFinder { offset, result: None };
    finder.visit_block(block);
    finder.result
}

struct StringArgFinder<'ast> {
    offset: u32,
    result: Option<StringArgSite<'ast>>,
}

impl<'ast> Visitor<'ast> for StringArgFinder<'ast> {
    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.try_record(call);
        self.walk_function_call(call);
    }
}

impl<'ast> StringArgFinder<'ast> {
    /// Strictly inside the quotes, so a cursor next to the literal doesn't
    /// trigger value completion.
    const fn contains(&self, span: Span) -> bool {
        self.offset > span.start && self.offset < span.end
    }

    fn try_record(&mut self, call: &'ast FunctionCall) {
        let literal = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                args.iter().enumerate().find_map(|(index, arg)| {
                    if let Expression::StringLiteral(token) = arg
                        && self.contains(token.span)
                    {
                        Some((index, token.span))
                    } else {
                        None
                    }
                })
            }
            FunctionArgs::StringLiteral(token) if self.contains(token.span) => {
                Some((0, token.span))
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => None,
        };
        let Some((arg_index, span)) = literal else {
            return;
        };
        // Nested calls visit outer-first; the innermost match wins.
        self.result = Some(StringArgSite { call, arg_index, literal_span: span });
    }
}

/// The local symbol at a byte offset and the name span the offset sits in:
/// a reference to the local, or its declaration.
///
/// `None` for globals, keywords, and non-names. Matching is by span through
/// the scope tree, so shadowed names map to the right declaration.
pub(crate) fn local_at(tree: &ScopeTree, offset: u32) -> Option<(SymbolId, Span)> {
    if let Some(reference) =
        tree.references.iter().find(|reference| touches(reference.span, offset))
    {
        return Some((reference.resolved?, reference.span));
    }
    tree.symbols
        .iter()
        .find(|symbol| touches(symbol.definition_span, offset))
        .map(|symbol| (symbol.id, symbol.definition_span))
}
