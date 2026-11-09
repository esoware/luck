use core::slice;

use luck_ast::{Block, Expression, FieldAccess, FunctionArgs, FunctionCall, Literal, Var, Visitor};
use luck_semantic::{ScopeTree, SymbolId, SymbolKind};
use luck_token::{CommentKind, Span, Token, TokenKind, offset_to_index};

use crate::document::DocumentState;

/// Whether `offset` falls inside `span`, either end included, so a cursor
/// just past a name still counts as on it.
pub(crate) const fn touches(span: Span, offset: u32) -> bool {
    span.start <= offset && offset <= span.end
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKind {
    Code,
    Comment,
    String,
}

pub(crate) fn text_kind_at(document: &DocumentState, offset: u32) -> TextKind {
    let comments = &document.parsed.comments;
    let at_offset = comments.get(comments.partition_point(|comment| comment.span.end < offset));
    let is_in_comment = at_offset.is_some_and(|comment| {
        // A line comment runs through the end of its line, so typing there
        // stays inside it. A block comment closes at its `]]`.
        let has_room = match comment.kind {
            CommentKind::Line | CommentKind::Shebang => offset <= comment.span.end,
            CommentKind::SingleLineBlock | CommentKind::MultiLineBlock => offset < comment.span.end,
        };
        comment.span.start < offset && has_room
    });
    if is_in_comment {
        return TextKind::Comment;
    }
    let tokens = document.tokens();
    let enclosing = tokens.get(tokens.partition_point(|token| token.span.end <= offset));
    let is_in_string = enclosing.is_some_and(|token| {
        token.span.start < offset
            && matches!(
                token.kind,
                TokenKind::StringLiteral(_)
                    | TokenKind::InterpolatedStringBegin(_)
                    | TokenKind::InterpolatedStringMiddle(_)
                    | TokenKind::InterpolatedStringEnd(_)
            )
    });
    if is_in_string {
        return TextKind::String;
    }
    // An unterminated string or block comment is a lex error over its text
    // rather than a token, so its opener is what tells the two apart.
    document
        .parsed
        .errors
        .iter()
        .find_map(|e| {
            (e.span.start < offset && offset <= e.span.end)
                .then(|| unterminated_text_kind(e.span.text(document.text())))
        })
        .unwrap_or(TextKind::Code)
}

fn unterminated_text_kind(text: &str) -> TextKind {
    if text.starts_with("--") {
        TextKind::Comment
    } else if text.starts_with(['"', '\'', '`']) || text.starts_with("[[") || text.starts_with("[=")
    {
        TextKind::String
    } else {
        TextKind::Code
    }
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

/// The `Name(.Name)*` chain `tokens` ends with, root first, and the root's
/// span. `None` when anything else, such as a call, breaks the chain.
pub(crate) fn dotted_chain_ending_tokens(tokens: &[Token]) -> Option<(Vec<&str>, Span)> {
    let mut segments: Vec<&str> = Vec::new();
    let mut remaining_tokens = tokens;
    loop {
        let (base, before_base) = remaining_tokens.split_last()?;
        let TokenKind::Identifier(base_name) = &base.kind else {
            return None;
        };
        segments.push(base_name.as_str());
        if let Some((Token { kind: TokenKind::Dot, .. }, before_dot)) = before_base.split_last() {
            remaining_tokens = before_dot;
        } else {
            segments.reverse();
            return Some((segments, base.span));
        }
    }
}

#[derive(Debug)]
pub(crate) enum CursorNode<'ast> {
    Identifier {
        name: &'ast str,
        span: Span,
    },
    /// A dotted path like `string.format`, of name-only segments.
    DottedPath {
        path: Vec<&'ast str>,
        root_span: Span,
        full_span: Span,
    },
    /// Path is empty for indirect callees.
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

    fn visit_function_call(&mut self, call: &'ast FunctionCall) {
        self.record_call(call);
        self.walk_function_call(call);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CallSite<'ast> {
    /// The call node, so consumers can resolve the callee semantically
    /// (shaped locals, shadow checks).
    pub call: &'ast FunctionCall,
    pub active_param: usize,
}

pub(crate) fn find_call_site_at(document: &DocumentState, offset: u32) -> Option<CallSite<'_>> {
    let mut finder =
        CallSiteFinder { offset, source: document.text(), tokens: document.tokens(), result: None };
    finder.visit_block(&document.parsed.block);
    finder.result
}

struct CallSiteFinder<'document> {
    offset: u32,
    source: &'document str,
    tokens: &'document [Token],
    result: Option<CallSite<'document>>,
}

impl<'document> Visitor<'document> for CallSiteFinder<'document> {
    fn visit_function_call(&mut self, call: &'document FunctionCall) {
        self.try_record(call);
        self.walk_function_call(call);
    }
}

impl<'document> CallSiteFinder<'document> {
    fn try_record(&mut self, call: &'document FunctionCall) {
        // `f"..."` and `f{...}` have no parens for the cursor to be inside.
        let FunctionArgs::Parenthesized { span: parens, args } = &call.args else {
            return;
        };
        // An unclosed list ends where the parser stopped, so the end still
        // counts as inside it. Past a real `)` the cursor has left the call.
        let is_closed = self.source.as_bytes().get(offset_to_index(parens.end.saturating_sub(1)))
            == Some(&b')');
        if self.offset <= parens.start
            || self.offset > parens.end
            || (is_closed && self.offset == parens.end)
        {
            return;
        }
        let active_param =
            args.iter().filter(|arg| self.is_followed_by_comma_before_cursor(arg)).count();
        self.result = Some(CallSite { call, active_param });
    }

    /// Whether the token after `arg` is a comma the cursor is past. Reading
    /// the next token, rather than counting commas, can't mistake one inside
    /// a nested argument for a separator of this list.
    fn is_followed_by_comma_before_cursor(&self, arg: &Expression) -> bool {
        let next_token = self.tokens.partition_point(|token| token.span.start < arg.span().end);
        self.tokens.get(next_token).is_some_and(|token| {
            matches!(token.kind, TokenKind::Comma) && token.span.end <= self.offset
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StringArgSite<'ast> {
    pub call: &'ast FunctionCall,
    pub arg_index: usize,
    pub literal: &'ast Literal,
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
        let site = match &call.args {
            FunctionArgs::Parenthesized { args, .. } => {
                args.iter().enumerate().find_map(|(index, arg)| {
                    if let Expression::StringLiteral(literal) = arg
                        && self.contains(literal.span)
                    {
                        Some((index, literal))
                    } else {
                        None
                    }
                })
            }
            FunctionArgs::StringLiteral(literal) if self.contains(literal.span) => {
                Some((0, literal))
            }
            FunctionArgs::StringLiteral(_) | FunctionArgs::TableConstructor(_) => None,
        };
        let Some((arg_index, literal)) = site else {
            return;
        };
        // Nested calls visit outer-first; the innermost match wins.
        self.result = Some(StringArgSite { call, arg_index, literal });
    }
}

/// The local symbol at a byte offset and the name span the offset sits in:
/// a reference to the local, or its declaration.
///
/// `None` for globals, keywords, and non-names. Matching is by span through
/// the scope tree, so shadowed names map to the right declaration. A name
/// parse recovery made up is nothing to find, and neither is the implicit
/// `self`'s declaration, which no token spells.
pub(crate) fn local_at(tree: &ScopeTree, offset: u32) -> Option<(SymbolId, Span)> {
    if let Some(reference) = tree
        .references
        .iter()
        .find(|reference| !reference.name.is_empty() && touches(reference.span, offset))
    {
        return Some((reference.resolved?, reference.span));
    }
    tree.symbols
        .iter()
        .find(|symbol| {
            symbol.kind != SymbolKind::ImplicitSelf
                && !symbol.name.is_empty()
                && touches(symbol.definition_span, offset)
        })
        .map(|symbol| (symbol.id, symbol.definition_span))
}
